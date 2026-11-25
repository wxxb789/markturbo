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
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
        ArtifactLens, ByteRange, ClarificationPriority, FindingKind, ReviewModelOutput,
        ReviewRequest, ReviewResult, ReviewScope, ReviewSource, ReviewStatus, SkillFilePayload,
        SourceAnchor, SourceLocation, SourceSnapshot,
        provider::{
            PreparedDocumentReviewRequest, PreparedReview, PreparedRevisionRequest,
            REVISION_REQUEST_TIMEOUT, ReviewError, ReviewLanguage, RevisionAnswer, RevisionAnswers,
            RevisionError, RevisionExecutionError,
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
use nsh_base::{
    alert_dialog,
    editor::{Editor, EditorApi},
};
use nsh_core::{file_dialog::SaveDialog, prelude::*};

mod revision_ledger;

use revision_ledger::{LedgerRefusal, RevisionLedger};

static FIRST_LAUNCH_CHECKED: AtomicBool = AtomicBool::new(false);

/// The status line while a proposal waits for decisions.
const REVISION_READY: &str =
    "Revision ready. Each accepted change enters the buffer at once; nothing is saved.";
/// The status line when there is no proposal to decide or finish.
const REVISION_UNAVAILABLE: &str = "Run Review first to prepare a Revision.";

/// Which panes a document shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Source,
    Preview,
    Web,
    Split,
    SplitWeb,
}

impl Pane {
    fn mounts_source(self) -> bool {
        matches!(self, Pane::Source | Pane::Split | Pane::SplitWeb)
    }

    pub(crate) fn of(stored: mt_core::settings::DocumentPane) -> Self {
        match stored {
            mt_core::settings::DocumentPane::Source => Pane::Source,
            mt_core::settings::DocumentPane::Preview => Pane::Preview,
            mt_core::settings::DocumentPane::Web => Pane::Web,
            mt_core::settings::DocumentPane::Split => Pane::Split,
            mt_core::settings::DocumentPane::SplitWeb => Pane::SplitWeb,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn stored(self) -> mt_core::settings::DocumentPane {
        match self {
            Pane::Source => mt_core::settings::DocumentPane::Source,
            Pane::Preview => mt_core::settings::DocumentPane::Preview,
            Pane::Web => mt_core::settings::DocumentPane::Web,
            Pane::Split => mt_core::settings::DocumentPane::Split,
            Pane::SplitWeb => mt_core::settings::DocumentPane::SplitWeb,
        }
    }
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
    pub conflict: Signal<bool>,
    pub untitled: bool,
    pub recovery_key: RecoveryKey,
    pub loaded: Rc<RefCell<LoadedFile>>,
    /// The latest Reload this tab asked for: an older request's answer that
    /// comes back later writes nothing.
    reload: Rc<Cell<u64>>,
}

impl PartialEq for Tab {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// A mounted source editor's handle and the tab it edits.
type SourceEditor = (u64, ComponentRef<Editor>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusSurface {
    Editor,
    Review,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusKind {
    Info,
    Success,
    Error,
}

pub(crate) type StatusAction = Rc<dyn Fn()>;

#[derive(Clone, Default)]
pub struct StatusMessage {
    pub(crate) text: SharedString,
    pub(crate) count: Option<SharedString>,
    pub(crate) action: Option<(SharedString, StatusAction)>,
}

impl PartialEq for StatusMessage {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.count == other.count
            && self.action.as_ref().map(|(label, _)| label)
                == other.action.as_ref().map(|(label, _)| label)
    }
}

impl StatusMessage {
    pub(crate) fn plain(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            count: None,
            action: None,
        }
    }

    pub(crate) fn counted(text: impl Into<SharedString>, count: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            count: Some(count.into()),
            action: None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// The shell's state.
#[derive(Clone)]
pub struct AppState {
    pub root: Signal<Option<PathBuf>>,
    pub entries: Signal<Vec<Entry>>,
    pub tabs: Signal<Vec<Tab>>,
    pub active: Signal<Option<u64>>,
    pub status: Signal<StatusMessage>,
    pub(crate) status_surface: Signal<StatusSurface>,
    pub(crate) status_error: Signal<bool>,
    /// Distinguishes notifications even when their text is identical, so an
    /// older fade cannot clear a newer status line.
    pub(crate) status_generation: Signal<u64>,
    /// Whether the status line's message holds until the next one (an error,
    /// a refusal, a wait) rather than fading after a moment (something done).
    /// Write both through [`AppState::notify`] and [`AppState::notify_held`].
    pub status_held: Signal<bool>,
    pub search_query: Signal<String>,
    pub search_results: Signal<Vec<SearchHit>>,
    pub search_ran: Signal<bool>,
    pub search_truncated: Signal<bool>,
    pub search_busy: Signal<bool>,
    pub(crate) palette_open: Signal<bool>,
    /// Whether the titlebar menu is open: the first layer of the fixed
    /// Escape order.
    pub title_menu_open: Signal<bool>,
    pub settings: Signal<SettingsData>,
    pub settings_open: Signal<bool>,
    pub settings_credential: Signal<String>,
    pub settings_notice: Signal<SharedString>,
    pub(crate) settings_notice_error: Signal<bool>,
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
    pub(crate) revision_status_error: Signal<bool>,
    pub revision_busy: Signal<bool>,
    /// How many hunks the last `Done` discarded undecided (never written);
    /// zero when there is nothing to report.
    pub revision_not_applied: Signal<usize>,
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
    /// The companion CJK face the settings page downloads on request.
    pub companion_font: crate::ui::fonts::Companion,
    owner: Owner,
    vault: CredentialVault,
    usage: Rc<RefCell<UsageStore>>,
    pending_review: Rc<RefCell<Option<PreparedDocumentReviewRequest>>>,
    review_origin: Rc<RefCell<Option<ReviewOrigin>>>,
    /// The non-empty source selection captured, as byte offsets into the
    /// frozen text, when the waiting Review consent was prepared: the
    /// `Selection` scope the card may switch to. `None` offers no selection.
    review_selection: Rc<Cell<Option<(usize, usize)>>>,
    /// The source editor `ui::document` mounted last, with the tab it edits.
    /// Its selection is read live when a Review is prepared; once that
    /// editor is unmounted (another pane or tab) its handle reaches nothing,
    /// so a selection nobody sees is never reviewed.
    source_editor: Rc<Cell<Option<SourceEditor>>>,
    /// The tab a sent Review request belongs to, until its answer lands or
    /// the request stops mattering (replaced or withdrawn). It outlives the
    /// tab being on screen: switching tabs must not drop an answer nobody
    /// has read yet.
    review_in_flight: Rc<RefCell<Option<ReviewOrigin>>>,
    review_binding: Rc<RefCell<Option<ReviewOrigin>>>,
    revision_context: Rc<RefCell<Option<RevisionContext>>>,
    /// Bumped whenever the revision context is replaced or reset: a Revision
    /// answer sent under an older generation belongs to no live proposal and
    /// is dropped unread.
    revision_generation: Rc<Cell<u64>>,
    revision_cancellation: Rc<RefCell<Option<Arc<AtomicBool>>>>,
    /// Bumped by every context resolution and reset: only the latest
    /// resolution lands, so a slower one for the document that was active
    /// before cannot replace the chain of the one active now.
    context_generation: Rc<Cell<u64>>,
    pending_revision: Rc<RefCell<Option<PreparedRevisionRequest>>>,
    /// Where the live proposal's hunks sit in its tab's text: the one
    /// authority that writes an accepted change, or takes one back.
    revision_ledger: Rc<RefCell<Option<RevisionLedger>>>,
    recovery: Option<Rc<RecoveryStore>>,
    recovery_tracks: Rc<RefCell<HashMap<u64, RecoveryTrack>>>,
    undo_hold: Rc<RefCell<Option<UndoHold>>>,
    undo_generation: Rc<Cell<u64>>,
    quit_discarded: Rc<Cell<bool>>,
    chosen_pane: Rc<Cell<Pane>>,
    /// The document view mounted last and the tab it belongs to: a retry
    /// waits for the tab it was asked to focus, never the ref it replaced.
    document_view: Rc<Cell<Option<(u64, ElementRef)>>>,
    guard_view: Rc<Cell<Option<ElementRef>>>,
    compare_view: Rc<Cell<Option<ElementRef>>>,
    palette_view: Rc<Cell<Option<ElementRef>>>,
    /// The latest folder open's request token: a slower earlier scan must
    /// not commit its root, files, context, recents or status.
    folder_request: Rc<Cell<u64>>,
    #[cfg(test)]
    pub(crate) focus_last: Signal<Option<FocusTarget>>,
    next_id: Rc<Cell<u64>>,
}

/// Checkpoint timing for one dirty tab.
struct RecoveryTrack {
    last_edit: Instant,
    last_checkpoint: Option<Instant>,
}

struct UndoHold {
    records: Vec<RecoveredRecord>,
    generation: u64,
    status_generation: u64,
    task: Spawned<()>,
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
    pub kind: GuardKind,
    pub choice: Signal<usize>,
    /// A save is going on: the guard's controls wait for it.
    pub busy: Signal<bool>,
    /// Why the guard is still up (a save that could not finish).
    pub notice: Signal<Option<SharedString>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    CloseTab(u64),
    QuitApp,
    ReloadFromDisk(u64),
    OverwriteDisk(u64),
}

impl GuardKind {
    pub fn choices(self) -> usize {
        match self {
            GuardKind::CloseTab(_) | GuardKind::QuitApp => GUARD_CHOICES,
            GuardKind::ReloadFromDisk(_) | GuardKind::OverwriteDisk(_) => 2,
        }
    }

    /// The control the keyboard starts on: Cancelling is the default, so the
    /// destructive choice is never one Enter away from the open.
    fn default_choice(self) -> usize {
        self.choices() - 1
    }
}

#[derive(Clone, PartialEq, Eq)]
enum SaveAsOutcome {
    Saved(u64),
    Cancelled,
    Failed(SharedString),
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
    /// The selected part of the reviewed document, alone.
    Selection,
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
    /// What leaves and why, in display order.
    pub items: Vec<OutboundItem>,
    /// Normally in-scope items that do not leave, in display order.
    pub stays: Vec<OutboundItem>,
}

/// The outbound scope a waiting Review consent can be rebound to: the spec's
/// `Selection · Document · Document + context`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewScopeChoice {
    /// Only the source selection captured when the Review was prepared.
    Selection,
    /// The whole document (or Agent Skill package).
    Document,
    /// The whole document with its selected Effective Agent Context sources.
    DocumentWithContext,
}

/// The source editor's primary selection in the editor's own coordinates:
/// a line from 0 and a column in UTF-16 code units, from where it started
/// (`anchor`) to where the caret is (`head`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceSelection {
    anchor: (u32, u32),
    head: (u32, u32),
}

impl SourceSelection {
    /// The selection as a byte range of `text`, ordered and clamped to the
    /// text; `None` when it selects nothing.
    fn byte_range(&self, text: &str) -> Option<(usize, usize)> {
        let anchor = byte_offset(text, self.anchor);
        let head = byte_offset(text, self.head);
        let (start, end) = (anchor.min(head), anchor.max(head));
        (start < end).then_some((start, end))
    }
}

/// The byte offset of an editor position (line from 0, UTF-16 column) in
/// `text`, clamped to the end of its line (before its line break) and to the
/// end of the text.
fn byte_offset(text: &str, (line, column): (u32, u32)) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match line_bounds(text, start).1 {
            Some(next) => start = next,
            None => return text.len(),
        }
    }
    let line_text = &text[start..line_bounds(text, start).0];
    let mut units = 0;
    for (offset, character) in line_text.char_indices() {
        if units >= column as usize {
            return start + offset;
        }
        units += character.len_utf16();
    }
    start + line_text.len()
}

/// Where the line starting at byte `start` of `text` ends (before its break)
/// and where the next line starts, if one does. Lines break as the source
/// editor breaks them: at `\r\n`, `\n` or a lone `\r`.
fn line_bounds(text: &str, start: usize) -> (usize, Option<usize>) {
    match text[start..].find(['\r', '\n']) {
        None => (text.len(), None),
        Some(at) => {
            let end = start + at;
            let next = if text[end..].starts_with("\r\n") {
                end + 2
            } else {
                end + 1
            };
            (end, Some(next))
        }
    }
}

/// Immutable inputs retained after a successful Review for a later Revision.
struct RevisionContext {
    tab_id: u64,
    /// The tab's source revision the Review read: once the text moves on,
    /// the Review no longer describes it and no Revision is prepared from it.
    revision: u64,
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
    /// Whether the reviewed Agent Skill package omitted a normally in-scope
    /// supporting file: the result is labelled partial, never complete.
    pub partial: bool,
}

/// A request to align one source line with the column's reading position.
/// The request scrolls the source view only; document contents are unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentFocus {
    /// The tab the line belongs to; another tab's request is ignored.
    pub tab: u64,
    /// The line, from 0.
    pub line: u32,
    /// Whether this request may glide; frequent and keyboard navigation cuts.
    pub animated: bool,
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
    /// The state over the platform's secure store; in a test build, over
    /// this process's in-memory store instead (the `test_vault` module).
    pub fn new() -> Self {
        #[cfg(not(any(test, feature = "test-vault")))]
        let vault = CredentialVault::production();
        #[cfg(any(test, feature = "test-vault"))]
        let vault = crate::test_vault::vault();
        Self::build(vault)
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
        #[cfg(test)]
        {
            crate::test_env::hermetic();
        }
        let settings = SettingsData::load();
        let chosen_pane = Pane::of(settings.document_pane);
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
        // With nothing to report the status line starts empty: the welcome's
        // own choices and its local-only line say how to begin, once.
        let status = StatusMessage::plain(notice.clone().unwrap_or_default());
        let usage = UsageStore::open(settings.usage_metrics_enabled);
        let state = Self {
            owner: Owner::current(),
            root: signal(None),
            entries: signal(Vec::new()),
            tabs: signal(Vec::new()),
            active: signal(None),
            status: signal(status),
            status_generation: signal(0_u64),
            status_held: signal(notice.is_some()),
            status_surface: signal(StatusSurface::Editor),
            status_error: signal(notice.is_some()),
            search_query: signal(String::new()),
            search_results: signal(Vec::new()),
            search_ran: signal(false),
            search_truncated: signal(false),
            search_busy: signal(false),
            palette_open: signal(false),
            title_menu_open: signal(false),
            settings: signal(settings),
            settings_open: signal(false),
            settings_credential: signal(String::new()),
            settings_notice: signal(SharedString::from("")),
            settings_notice_error: signal(false),
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
            revision_status_error: signal(false),
            revision_busy: signal(false),
            revision_not_applied: signal(0),
            vault,
            review_generation: signal(0),
            guard: signal(None),
            conflict_compare: signal(None),
            conflict_difference: signal(0),
            conflict_disks: signal(Vec::new()),
            companion_font: crate::ui::fonts::Companion::new(),
            usage: Rc::new(RefCell::new(usage)),
            pending_review: Rc::new(RefCell::new(None)),
            review_origin: Rc::new(RefCell::new(None)),
            review_selection: Rc::new(Cell::new(None)),
            source_editor: Rc::new(Cell::new(None)),
            review_in_flight: Rc::new(RefCell::new(None)),
            review_binding: Rc::new(RefCell::new(None)),
            revision_context: Rc::new(RefCell::new(None)),
            revision_generation: Rc::new(Cell::new(0)),
            revision_cancellation: Rc::default(),
            context_generation: Rc::new(Cell::new(0)),
            pending_revision: Rc::new(RefCell::new(None)),
            revision_ledger: Rc::new(RefCell::new(None)),
            recovery,
            recovery_tracks: Rc::new(RefCell::new(HashMap::new())),
            undo_hold: Rc::new(RefCell::new(None)),
            undo_generation: Rc::new(Cell::new(0)),
            quit_discarded: Rc::new(Cell::new(false)),
            chosen_pane: Rc::new(Cell::new(chosen_pane)),
            document_view: Rc::new(Cell::new(None)),
            guard_view: Rc::new(Cell::new(None)),
            compare_view: Rc::new(Cell::new(None)),
            palette_view: Rc::new(Cell::new(None)),
            folder_request: Rc::new(Cell::new(0)),
            #[cfg(test)]
            focus_last: signal(None),
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
        self.set_settings_notice(
            crate::i18n::t(self.settings.get().language, key),
            matches!(
                key,
                crate::i18n::Key::UsageUnavailable
                    | crate::i18n::Key::UsageConsentSaveFailed
                    | crate::i18n::Key::UsageExportFailed
            ),
        );
    }

    pub(crate) fn set_settings_notice(&self, message: impl Into<SharedString>, error: bool) {
        self.settings_notice_error.set(error);
        self.settings_notice.set(message.into());
    }

    /// Say something the app did on the status line; it fades after a moment.
    pub fn notify(&self, message: impl Into<SharedString>) {
        self.notify_at(StatusSurface::Editor, StatusKind::Success, message);
    }

    /// Say why something did not happen (an error, a refusal) or what is
    /// still happening: the line holds it until the next message.
    pub fn notify_held(&self, message: impl Into<SharedString>) {
        self.notify_at(StatusSurface::Editor, StatusKind::Info, message);
    }

    pub(crate) fn notify_error(&self, message: impl Into<SharedString>) {
        // An error belongs to the surface the user is on: with Settings open,
        // the editor's status line is not mounted to show it.
        let surface = if self.settings_open.get() {
            StatusSurface::Settings
        } else {
            StatusSurface::Editor
        };
        self.notify_at(surface, StatusKind::Error, message);
    }

    fn notify_review_error(&self, message: impl Into<SharedString>) {
        let surface = if self.review_open.get_untracked()
            || self.review_unavailable.get_untracked().is_some()
        {
            StatusSurface::Review
        } else {
            StatusSurface::Editor
        };
        self.notify_at(surface, StatusKind::Error, message);
    }

    pub(crate) fn notify_at(
        &self,
        surface: StatusSurface,
        kind: StatusKind,
        message: impl Into<SharedString>,
    ) {
        self.publish(surface, kind, StatusMessage::plain(message));
    }

    /// A status whose trailing count changes: the count renders in the mono family.
    pub(crate) fn notify_count_at(
        &self,
        surface: StatusSurface,
        kind: StatusKind,
        text: impl Into<SharedString>,
        count: impl Into<SharedString>,
    ) {
        self.publish(surface, kind, StatusMessage::counted(text, count));
    }

    fn publish(&self, mut surface: StatusSurface, kind: StatusKind, mut message: StatusMessage) {
        // A pending Undo stays on the line: a newer message carries it on
        // until its deadline, when the slot is freed or taken. Its line is
        // the surface the user sees, computed from the UI state, never the
        // surface the notice was first routed to (contract amendment 18).
        let live_undo = self.undo_hold.borrow().as_ref().map(|hold| hold.generation);
        if live_undo.is_some() {
            surface = self.undo_surface();
        }
        if let Some(generation) = live_undo {
            let state = self.clone();
            let label = SharedString::from(s::undo(self.settings.get().language));
            message.action = Some((label, Rc::new(move || state.undo_pending(generation))));
        }
        self.status_surface.set(surface);
        self.status_error.set(kind == StatusKind::Error);
        self.status_held.set(kind != StatusKind::Success);
        self.status_generation
            .set(self.status_generation.get_untracked().wrapping_add(1));
        self.status.set(message);
        if let Some(hold) = self.undo_hold.borrow_mut().as_mut() {
            hold.status_generation = self.status_generation.get_untracked();
        }
    }

    /// Where a live Undo renders: Settings when it is open, otherwise the
    /// Review line while its panel is open, otherwise the editor's
    /// (contract amendment 18). The UI state decides, not the notice.
    fn undo_surface(&self) -> StatusSurface {
        if self.settings_open.get_untracked() {
            StatusSurface::Settings
        } else if self.review_open.get_untracked()
            || self.review_unavailable.get_untracked().is_some()
        {
            StatusSurface::Review
        } else {
            StatusSurface::Editor
        }
    }

    /// Move a live Undo to the surface the user now sees, when a surface was
    /// opened or closed without a new notice. The hold's deadline must stay
    /// reachable until it fires; lanes that open Settings or the Review
    /// panel call this with their transition.
    pub(crate) fn sync_undo_surface(&self) {
        if self.undo_hold.borrow().is_some() {
            self.status_surface.set(self.undo_surface());
        }
    }

    /// `Opened plan.md`: a done message naming its file or folder, never the
    /// path it sits in.
    fn notify_named(&self, key: crate::i18n::Key, path: &Path) {
        let language = self.settings.get().language;
        self.notify(format!(
            "{} {}",
            crate::i18n::t(language, key),
            crate::i18n::file_name(path)
        ));
    }

    /// `Save failed: <reason>`, localized, the error in its Display form.
    pub(crate) fn notify_failure(&self, key: crate::i18n::Key, error: &dyn std::fmt::Display) {
        let language = self.settings.get().language;
        let label = crate::i18n::t(language, key);
        self.notify_error(match language {
            Language::English => format!("{label}: {error}"),
            Language::Chinese => format!("{label}：{error}"),
        });
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

    /// The walk runs off the UI thread; each open carries a request token,
    /// and only the latest may commit.
    pub fn open_folder(&self, path: PathBuf) {
        let request = self.folder_request.get().wrapping_add(1);
        self.folder_request.set(request);
        self.notify_held(SharedString::from(s::opening_folder(
            self.settings.get().language,
        )));
        let state = self.clone();
        self.spawn_task(async move {
            let scanned = blocking({
                let path = path.clone();
                move || {
                    #[cfg(any(test, feature = "test-vault"))]
                    crate::test_vault::wait_for_read(&path);
                    tree::read_dir_deep(&path, 4)
                }
            })
            .await;
            if state.folder_request.get() != request {
                // A superseded open writes nothing; the test harness learns
                // the stale scan returned this way.
                #[cfg(any(test, feature = "test-vault"))]
                crate::test_vault::note_superseded_open(&path);
                return;
            }
            match scanned {
                Ok(Ok(nodes)) => {
                    let mut entries = Vec::new();
                    flatten(&nodes, 0, &mut entries);
                    state.root.set(Some(path.clone()));
                    state.entries.set(entries);
                    state.reset_context();
                    if state.context_open.get() {
                        state.refresh_context();
                    }
                    state.record_recent(&path, RecentTargetKind::Workspace);
                    state.notify_named(crate::i18n::Key::Opened, &path);
                }
                Ok(Err(error)) => state.notify_failure(crate::i18n::Key::OpenFolderFailed, &error),
                Err(error) => state.notify_failure(crate::i18n::Key::OpenFolderFailed, &error),
            }
        });
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
                    state.notify_failure(crate::i18n::Key::OpenFailed, &error);
                    return;
                }
                Err(error) => {
                    state.notify_failure(crate::i18n::Key::OpenFailed, &error);
                    return;
                }
            };
            let opened = loaded.path.clone();
            state.add_tab(loaded);
            state.record_usage(UsageEvent::ArtifactOpened);
            state.notify_named(crate::i18n::Key::Opened, &opened);
        });
    }

    /// Whether this tab holds live Review or Revision work: a request out for
    /// it, the answer bound to it, or the Revision a Review prepared. The
    /// work travels with the tab, not with what is on screen — switching tabs
    /// does not move it, and replacing the tab would drop it.
    pub fn holds_live_model_output(&self, id: u64) -> bool {
        let owns = |origin: &Option<ReviewOrigin>| origin.is_some_and(|origin| origin.tab_id == id);
        // The answer is the tab's while its result is the one on screen: a
        // closed Review keeps its binding but holds nothing.
        let shown = self
            .review_view
            .get()
            .is_some_and(|view| view.tab_id == Some(id));
        let binding = *self.review_binding.borrow();
        let answer = shown
            && binding.is_some_and(|binding| {
                binding.tab_id == id
                    // The same test `note_edit` uses to call a review stale:
                    // the text has not moved on since the model read it.
                    && self
                        .tabs
                        .get()
                        .iter()
                        .find(|tab| tab.id == id)
                        .is_some_and(|tab| tab.revision.get() == binding.snapshot.revision)
            });
        let revision = self.revision_context.borrow();
        let revision_work = match revision.as_ref() {
            Some(context) => context.tab_id == id,
            // Hunks and questions clear with the context; without one they
            // are the tab on screen's (a Revision still coming back).
            None => {
                self.active.get() == Some(id)
                    && (!self.revision_hunks.get().is_empty()
                        || !self.revision_questions.get().is_empty())
            }
        };
        owns(&self.review_origin.borrow())
            || owns(&self.review_in_flight.borrow())
            || answer
            || revision_work
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
                state.notify_held(SharedString::from(
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

    /// The smallest free number, never the internal tab id (contract note,
    /// decision 22).
    fn next_untitled(&self, language: Language, exclude: Option<u64>) -> (u64, SharedString) {
        let used: HashSet<u64> = self
            .tabs
            .get()
            .iter()
            .filter(|tab| tab.untitled && Some(tab.id) != exclude)
            .filter_map(|tab| untitled_number(&tab.title))
            .collect();
        let mut number = 1;
        while used.contains(&number) {
            number += 1;
        }
        (number, SharedString::from(s::untitled(language, number)))
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
        let (_, title) = self.next_untitled(self.settings.get().language, None);
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
                pane: signal(self.chosen_pane.get()),
                conflict: signal(false),
                untitled: true,
                recovery_key,
                loaded: Rc::new(RefCell::new(loaded)),
                reload: Rc::new(Cell::new(0)),
            });
        });
        if dirty {
            self.note_edit(id);
        }
        self.set_active(id);
    }

    fn add_tab(&self, loaded: LoadedFile) -> u64 {
        self.add_tab_with(loaded, false)
    }

    fn add_tab_with(&self, loaded: LoadedFile, dirty: bool) -> u64 {
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
                pane: signal(self.chosen_pane.get()),
                conflict: signal(false),
                untitled: false,
                recovery_key,
                loaded: Rc::new(RefCell::new(loaded)),
                reload: Rc::new(Cell::new(0)),
            });
        });
        self.set_active(id);
        id
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
        self.context_generation
            .set(self.context_generation.get().wrapping_add(1));
        self.context_busy.set(false);
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
        let generation = self.context_generation.get().wrapping_add(1);
        self.context_generation.set(generation);
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
            if state.context_generation.get() != generation {
                return;
            }
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
        let language = self.settings.get().language;
        match crate::app_paths::bundled_sample_dir() {
            Ok(directory) => self.open_folder(directory),
            Err(error) => self.notify_error(s::sample_unavailable(language, &error)),
        }
    }

    /// Ask the column to align `line` in `tab_id` with its reading position.
    /// Use [`Self::focus_document_line_instant`] for frequent or keyboard navigation.
    pub fn focus_document_line(&self, tab_id: u64, line: u32) {
        self.request_document_line(tab_id, line, true);
    }

    /// Ask the source column to level `line` (from 0) of tab `tab_id` at once.
    /// Use for frequent or keyboard-driven navigation.
    pub fn focus_document_line_instant(&self, tab_id: u64, line: u32) {
        self.request_document_line(tab_id, line, false);
    }

    fn request_document_line(&self, tab_id: u64, line: u32, animated: bool) {
        let request = self.document_focus.get().request.wrapping_add(1);
        self.document_focus.set(DocumentFocus {
            tab: tab_id,
            line,
            animated,
            request,
        });
    }

    /// Remember that a tab was edited, so its next checkpoint waits for a
    /// short idle window instead of copying on every keystroke.
    pub fn note_edit(&self, id: u64) {
        self.observe_revision_edit(id);
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
            self.notify_error(s::checkpoint_failed(self.settings.get().language, &error));
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
                let (_, title) = self.next_untitled(self.settings.get().language, Some(id));
                self.tabs.update(|tabs| {
                    if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.untitled = true;
                        tab.title = title;
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
                self.note_conflict(&tab);
            }
            self.notify_held(SharedString::from(s::recovered_conflict(
                self.settings.get().language,
            )));
        }
        self.recovery_records
            .update(|records| records.retain(|record| record.record.key != key));
        self.record_usage(UsageEvent::RecoveryRestored);
    }

    pub fn discard_recovered(&self, record: &RecoveredRecord) {
        self.begin_undo(vec![record.clone()]);
    }

    pub fn restore_all_recovered(&self) {
        for record in self.recovery_records.get() {
            self.restore_recovered(&record);
        }
    }

    pub fn discard_all_recovered(&self) {
        self.begin_undo(self.recovery_records.get());
    }

    /// There is one pending slot: a second Tier-2 action commits the older
    /// hold at once, then starts its own.
    fn begin_undo(&self, records: Vec<RecoveredRecord>) {
        if records.is_empty() {
            return;
        }
        if let Some(previous) = self.undo_hold.borrow_mut().take() {
            let status_generation = previous.status_generation;
            previous.task.cancel();
            self.delete_recovered(&previous.records);
            self.end_undo_action(status_generation);
        }
        let keys: HashSet<RecoveryKey> = records
            .iter()
            .map(|record| record.record.key.clone())
            .collect();
        self.recovery_records
            .update(|current| current.retain(|record| !keys.contains(&record.record.key)));
        let generation = self.undo_generation.get().wrapping_add(1);
        self.undo_generation.set(generation);
        let state = self.clone();
        let task = self.owner.run(|| {
            spawn(async move {
                nsh_core::element::timer_until(
                    nsh_core::time::now() + Duration::from_millis(crate::ui::motion::UNDO_HOLD_MS),
                )
                .await;
                state.commit_undo(generation);
            })
        });
        *self.undo_hold.borrow_mut() = Some(UndoHold {
            records,
            generation,
            status_generation: 0,
            task,
        });
        self.publish(
            StatusSurface::Editor,
            StatusKind::Info,
            StatusMessage::plain(s::discarded_draft(self.settings.get().language)),
        );
    }

    /// The deadline task was already cancelled; its later fire on a taken
    /// hold is a no-op.
    fn undo_pending(&self, generation: u64) {
        let Some(hold) = self.undo_hold.borrow_mut().take() else {
            return;
        };
        if hold.generation != generation {
            *self.undo_hold.borrow_mut() = Some(hold);
            return;
        }
        hold.task.cancel();
        let keys: HashSet<RecoveryKey> = hold
            .records
            .iter()
            .map(|record| record.record.key.clone())
            .collect();
        let restored = hold.records;
        self.recovery_records.update(|records| {
            records.retain(|record| !keys.contains(&record.record.key));
            records.extend(restored);
            records.sort_by_key(|record| record.record.checkpointed_at);
        });
        self.end_undo_action(hold.status_generation);
    }

    fn commit_undo(&self, generation: u64) {
        let Some(hold) = self.undo_hold.borrow_mut().take() else {
            return;
        };
        if hold.generation != generation {
            *self.undo_hold.borrow_mut() = Some(hold);
            return;
        }
        self.delete_recovered(&hold.records);
        self.end_undo_action(hold.status_generation);
    }

    /// Drop the ended hold's Undo from the status line while it still names
    /// the message there; the generation bump lets the line fade on.
    fn end_undo_action(&self, status_generation: u64) {
        if self.status_generation.get_untracked() != status_generation {
            return;
        }
        self.status.update(|message| message.action = None);
        self.status_generation.update(|generation| *generation += 1);
    }

    /// Only the commit bound to the draft it captured may delete: a newer
    /// checkpoint under the same key means the path was reopened and edited,
    /// and a record the scan could not read cannot be confirmed as the
    /// captured one.
    fn delete_recovered(&self, records: &[RecoveredRecord]) {
        let Some(store) = &self.recovery else {
            return;
        };
        let Ok(scan) = store.recover() else {
            return;
        };
        for record in records {
            let replaced = scan.records.iter().any(|current| {
                current.record.key == record.record.key
                    && current.record.checkpointed_at != record.record.checkpointed_at
            });
            let unconfirmed = scan
                .issues
                .iter()
                .any(|issue| issue.key().is_some_and(|key| key == record.record.key));
            if !replaced && !unconfirmed {
                let _ = store.invalidate_and_delete(&record.record.key);
            }
        }
    }

    pub fn remove_recent_target(&self, path: &Path) {
        let mut settings = self.settings.get();
        if settings.remove_recent_target(path) {
            self.settings.set(settings);
            let _ = self.settings.get().try_save_general();
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
        let Some(root) = self.root.get() else {
            // Nothing ran: the palette must not read as an empty result set.
            self.notify_held(SharedString::from(s::search_needs_folder(
                self.settings.get().language,
            )));
            return;
        };
        self.search_ran.set(true);
        self.search_results.set(Vec::new());
        self.search_truncated.set(false);
        if !query.is_runnable() {
            self.search_busy.set(false);
            return;
        }
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
                Err(error) => state.notify_error(format!("Search failed: {error}")),
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
        self.open_guard(GuardKind::CloseTab(id));
    }

    fn open_guard(&self, kind: GuardKind) {
        // The guard outlives the control that asked for it (the card replaces
        // the tab's own row), so its signals are the app's.
        let default = kind.default_choice();
        let (choice, busy, notice) = self
            .owner
            .run(|| (signal(default), signal(false), signal(None)));
        self.guard.set(Some(GuardRequest {
            kind,
            choice,
            busy,
            notice,
        }));
    }

    /// A failed read reports why and leaves the comparison closed, so
    /// Compare cannot be a silent no-op.
    pub fn refresh_conflict_disk(&self, tab: &Tab) -> Result<(), std::io::Error> {
        let loaded = io::load(&tab.path)?;
        let id = tab.id;
        self.conflict_disks.update(|disks| {
            disks.retain(|(entry, _)| *entry != id);
            disks.push((id, loaded.text));
        });
        Ok(())
    }

    /// Note that `tab` differs from its file: its flag goes up and any disk
    /// text from an earlier conflict is dropped, so the comparison can never
    /// show — or mark lines against — a read that predates this conflict.
    /// The text is read when the comparison is asked for
    /// ([`Self::refresh_conflict_disk`]).
    fn note_conflict(&self, tab: &Tab) {
        tab.conflict.set(true);
        let id = tab.id;
        self.conflict_disks
            .update(|disks| disks.retain(|(entry, _)| *entry != id));
    }

    /// Run `f` under the state's own owner: what it makes (a new tab's
    /// signals, a new guard's) then belongs to the app, not to the element
    /// whose handler asked. A handler in a row that the action itself
    /// removes (a recovered record's Restore) would otherwise take the
    /// document's signals down with its owner.
    pub fn owned<R>(&self, f: impl FnOnce() -> R) -> R {
        self.owner.run(f)
    }

    pub fn guard_move(&self, delta: isize) {
        let Some(request) = self.guard.get() else {
            return;
        };
        let choices = request.kind.choices();
        let current = request.choice.get();
        let next = (current as isize + delta).rem_euclid(choices as isize) as usize;
        request.choice.set(next);
    }

    /// The guard's Enter: the selected control's decision.
    pub fn guard_activate(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        let choice = request.choice.get();
        match request.kind {
            GuardKind::CloseTab(_) | GuardKind::QuitApp => match choice {
                GUARD_DISCARD_CHOICE => self.guard_discard(),
                GUARD_CANCEL_CHOICE => self.guard_cancel(),
                _ => self.guard_save(),
            },
            GuardKind::ReloadFromDisk(id) => {
                if choice == 0 {
                    self.guard_reload(id);
                } else {
                    self.guard_cancel();
                }
            }
            GuardKind::OverwriteDisk(id) => {
                if choice == 0 {
                    self.guard_overwrite(id);
                } else {
                    self.guard_cancel();
                }
            }
        }
    }

    pub fn guard_save(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        if request.busy.get() {
            return;
        }
        let language = self.settings.get().language;
        match request.kind {
            GuardKind::CloseTab(id) => {
                self.set_active(id);
                let untitled = self
                    .tabs
                    .get()
                    .iter()
                    .find(|tab| tab.id == id)
                    .is_some_and(|tab| tab.untitled);
                if untitled {
                    request.busy.set(true);
                    request.notice.set(None);
                    let state = self.clone();
                    self.spawn_task(async move {
                        let outcome =
                            match state.tabs.get().iter().find(|tab| tab.id == id).cloned() {
                                Some(tab) => state.save_as_tab(tab).await,
                                None => SaveAsOutcome::Failed(SharedString::from(
                                    s::guard_save_failed(language),
                                )),
                            };
                        let Some(request) = state.guard.get() else {
                            return;
                        };
                        if request.kind != GuardKind::CloseTab(id) {
                            return;
                        }
                        request.busy.set(false);
                        match outcome {
                            SaveAsOutcome::Saved(new_id) => {
                                state.guard.set(None);
                                state.close_tab_now(new_id);
                                state.focus_editor();
                            }
                            SaveAsOutcome::Cancelled => request
                                .notice
                                .set(Some(SharedString::from(s::save_cancelled(language)))),
                            SaveAsOutcome::Failed(reason) => request.notice.set(Some(reason)),
                        }
                    });
                    return;
                }
                let saved = self.save_tab(id);
                if saved {
                    self.guard.set(None);
                    self.close_tab_now(id);
                    self.focus_editor();
                } else {
                    request
                        .notice
                        .set(Some(SharedString::from(s::guard_save_failed(language))));
                }
            }
            GuardKind::QuitApp => {
                let state = self.clone();
                request.busy.set(true);
                request.notice.set(None);
                self.spawn_task(async move {
                    // Quit re-reads the dirty tabs after every Save As and
                    // before closing: a tab that turned dirty while an
                    // earlier one saved (a translation return) is guarded
                    // too, and Cancel takes the guard away mid-flight.
                    loop {
                        if state.guard.get().is_none() {
                            return;
                        }
                        let Some(tab) =
                            state.tabs.get().iter().find(|tab| tab.dirty.get()).cloned()
                        else {
                            state.guard.set(None);
                            nsh_core::element::desktop::current_window().close();
                            return;
                        };
                        let outcome = if tab.untitled {
                            state.save_as_tab(tab).await
                        } else if state.save_tab(tab.id) {
                            SaveAsOutcome::Saved(tab.id)
                        } else {
                            SaveAsOutcome::Failed(SharedString::from(s::guard_save_failed(
                                language,
                            )))
                        };
                        if !matches!(outcome, SaveAsOutcome::Saved(_)) {
                            if let Some(request) = state.guard.get() {
                                request.busy.set(false);
                                let notice = match outcome {
                                    SaveAsOutcome::Cancelled => {
                                        SharedString::from(s::save_cancelled(language))
                                    }
                                    SaveAsOutcome::Failed(reason) => reason,
                                    SaveAsOutcome::Saved(_) => {
                                        SharedString::from(s::guard_save_failed(language))
                                    }
                                };
                                request.notice.set(Some(notice));
                            }
                            state.notify_error(s::quit_save_failed(language));
                            return;
                        }
                    }
                });
            }
            GuardKind::ReloadFromDisk(_) | GuardKind::OverwriteDisk(_) => {}
        }
    }

    /// The guard's Don't save: close the covered documents, retiring their
    /// recovery records.
    pub fn guard_discard(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        self.guard.set(None);
        match request.kind {
            GuardKind::CloseTab(id) => {
                self.close_tab_now(id);
                self.focus_editor();
            }
            GuardKind::QuitApp => {
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
            GuardKind::ReloadFromDisk(_) | GuardKind::OverwriteDisk(_) => {}
        }
    }

    pub fn guard_cancel(&self) {
        if self.guard.get().is_some() {
            self.guard.set(None);
        }
        if self.active.get().is_some() {
            self.focus_editor();
        }
    }

    pub(crate) fn guard_reload(&self, id: u64) {
        self.guard.set(None);
        let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id).cloned() else {
            return;
        };
        // The confirmation was for this revision: an edit made while the
        // disk was read refuses it, keeping the buffer and its recovery.
        let confirmed_revision = tab.revision.get();
        let request = tab.reload.get().wrapping_add(1);
        tab.reload.set(request);
        let state = self.clone();
        self.spawn_task(async move {
            let path = tab.path.clone();
            let answer = blocking({
                let path = path.clone();
                move || {
                    #[cfg(any(test, feature = "test-vault"))]
                    crate::test_vault::wait_for_read(&path);
                    io::load(&path)
                }
            })
            .await;
            // A newer Reload took this tab's slot while the disk was read:
            // every answer, a failure included, belongs to the request the
            // user replaced and writes nothing.
            if tab.reload.get() != request {
                #[cfg(any(test, feature = "test-vault"))]
                crate::test_vault::note_superseded_reload(&path);
                return;
            }
            let loaded = match answer {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    state.notify_failure(crate::i18n::Key::ReloadFailed, &error);
                    return;
                }
                Err(error) => {
                    state.notify_failure(crate::i18n::Key::ReloadFailed, &error);
                    return;
                }
            };
            let Some(current) = state
                .tabs
                .get()
                .iter()
                .find(|current| current.id == id)
                .cloned()
            else {
                return;
            };
            if current.revision.get() != confirmed_revision {
                state.note_conflict(&current);
                state.focus_editor();
                return;
            }
            state.adopt_disk_version(&current, loaded);
            state.notify_named(crate::i18n::Key::Reloaded, &current.path);
            state.focus_editor();
        });
    }

    pub(crate) fn guard_overwrite(&self, id: u64) {
        self.guard.set(None);
        let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id).cloned() else {
            return;
        };
        let state = self.clone();
        self.spawn_task(async move {
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
                    state.notify_named(crate::i18n::Key::Saved, &tab.path);
                }
                Err(error) => state.notify_failure(crate::i18n::Key::OverwriteFailed, &error),
            }
            state.focus_editor();
        });
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

    pub fn reload_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        // The card owns Enter once it shows; until then an Enter must not
        // reach the focused editor and change the text the decision is about.
        self.blur_source_editor();
        self.open_guard(GuardKind::ReloadFromDisk(tab.id));
    }

    fn adopt_disk_version(&self, tab: &Tab, loaded: LoadedFile) {
        let text = loaded.text.clone();
        *tab.loaded.borrow_mut() = loaded;
        // The retiring comparison must not diff the reloaded buffer.
        batch(|| {
            tab.text.set(SharedString::from(text));
            tab.dirty.set(false);
            tab.conflict.set(false);
        });
        tab.revision.update(|revision| *revision += 1);
        self.retire_tab_recovery(tab);
        self.recovery_tracks.borrow_mut().remove(&tab.id);
    }

    fn write_model_text(&self, tab: &Tab, text: String) {
        tab.text.set(SharedString::from(text));
        tab.dirty.set(true);
        tab.revision.update(|revision| *revision += 1);
        self.note_edit(tab.id);
    }

    pub fn overwrite_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        // The card owns Enter once it shows; until then an Enter must not
        // reach the focused editor and change the text the decision is about.
        self.blur_source_editor();
        self.open_guard(GuardKind::OverwriteDisk(tab.id));
    }

    /// A picked destination another tab has open is refused: two tabs never
    /// edit one file.
    pub fn save_as_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let state = self.clone();
        self.spawn_task(async move {
            state.save_as_tab(tab).await;
        });
    }

    async fn save_as_tab(&self, tab: Tab) -> SaveAsOutcome {
        let language = self.settings.get().language;
        let picked = pick_save_path(
            SaveDialog::new()
                .title(crate::i18n::t(
                    self.settings.get().language,
                    crate::i18n::Key::SaveAsPicker,
                ))
                .file_name(tab.title.to_string())
                .filter(nsh_core::file_dialog::FileFilter::new(
                    "Markdown",
                    ["md", "markdown"],
                )),
        );
        // Test-only: the picker was asked before this stop and the answer is
        // not read until the test releases it.
        #[cfg(any(test, feature = "test-vault"))]
        let _ = blocking(crate::test_vault::wait_for_save_gate).await;
        match picked.await {
            Ok(Some(path)) => {
                if !tab.untitled && path == tab.path {
                    // Its own file: a plain Save, with its conflict check.
                    return if self.save_tab(tab.id) {
                        SaveAsOutcome::Saved(tab.id)
                    } else {
                        SaveAsOutcome::Failed(SharedString::from(s::guard_save_failed(language)))
                    };
                }
                if self
                    .tabs
                    .get()
                    .iter()
                    .any(|other| other.id != tab.id && other.path == path)
                {
                    // The file's name, not the folder it sits in.
                    let name = crate::i18n::file_name(&path);
                    let reason =
                        crate::i18n::save_as_path_already_open_message(language, Path::new(&name));
                    self.notify_held(reason.clone());
                    return SaveAsOutcome::Failed(SharedString::from(reason));
                }
                let text = tab.text.get().to_string();
                let (newline, had_bom) = {
                    let loaded = tab.loaded.borrow();
                    (loaded.newline, loaded.had_bom)
                };
                let saved = match io::save_as(&path, &text, newline, had_bom) {
                    // The native dialog already asked before this existing
                    // file was chosen: replace the entry it showed.
                    Err(SaveError::DestinationExists) => {
                        io::overwrite_as(&path, &text, newline, had_bom)
                    }
                    saved => saved,
                };
                match saved {
                    Ok(loaded) => {
                        self.close_tab_now(tab.id);
                        let new_id = self.add_tab(loaded);
                        self.record_usage(UsageEvent::ArtifactSaved);
                        SaveAsOutcome::Saved(new_id)
                    }
                    Err(error) => {
                        // The real I/O reason is the failed save's answer: the
                        // guard shows it, not a generic line.
                        let reason = s::save_as_write_failed(language, &error);
                        self.notify_error(reason.clone());
                        SaveAsOutcome::Failed(SharedString::from(reason))
                    }
                }
            }
            Ok(None) => SaveAsOutcome::Cancelled,
            Err(error) => {
                let reason = s::save_as_picker_failed(language, &error);
                self.notify_error(reason.clone());
                SaveAsOutcome::Failed(SharedString::from(reason))
            }
        }
    }

    pub fn set_pane(&self, id: u64, pane: Pane) {
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id) {
            tab.pane.set(pane);
            self.chosen_pane.set(pane);
            // The lib-test binary shares one settings dir; a pane choice must
            // not follow the next test's state into a different pane.
            #[cfg(not(test))]
            {
                self.settings
                    .update(|settings| settings.document_pane = pane.stored());
                let _ = self.settings.get().try_save_general();
            }
        }
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
            let language = self.settings.get().language;
            self.notify_held(SharedString::from(s::untitled_save_first(language)));
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
                self.notify_named(crate::i18n::Key::Saved, &tab.path);
                true
            }
            Err(SaveError::Conflict) => {
                self.note_conflict(&tab);
                self.notify_held(SharedString::from(crate::i18n::t(
                    self.settings.get().language,
                    crate::i18n::Key::SaveConflictDetail,
                )));
                false
            }
            Err(error) => {
                self.notify_failure(crate::i18n::Key::SaveFailed, &error);
                false
            }
        }
    }

    /// Ask before discarding every dirty buffer, then close the window. Quit
    /// and every user close of the window (its close button, Alt+F4: `app.rs`
    /// turns `close_on_press` off and routes `on_close_press` here) take this
    /// path; the in-app guard (`ui::notices`) takes the decision. A dirty
    /// buffer is checkpointed before the guard as a best-effort extra guard
    /// against a failed save.
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
        self.open_guard(GuardKind::QuitApp);
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
            self.notify_held(SharedString::from("No translatable text in this document."));
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
                self.notify_error(format!("Translation unavailable: {error}"));
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
                    state.notify_error(format!("Translation unavailable: {error}"));
                    return;
                }
            };
            state.translation_busy.set(true);
            state.notify_held(SharedString::from(
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
                        state.notify_held(SharedString::from(
                            crate::i18n::t(language, crate::i18n::Key::ModelRequestDocumentClosed)
                                .to_owned(),
                        ));
                        return;
                    };
                    if origin.snapshot.revision != tab.revision.get() {
                        state.record_usage(UsageEvent::StaleResultBlocked);
                        state.notify_held(SharedString::from(
                            crate::i18n::t(language, crate::i18n::Key::ModelRequestDocumentChanged)
                                .to_owned(),
                        ));
                        return;
                    }
                    state.write_model_text(&tab, translated);
                    state.notify(format!("Translated {segments} segment(s) into {target}."));
                }
                Ok(Err(error)) => {
                    if let Some(error) = error.downcast_ref::<TranslationError>() {
                        state.record_translation_failure(error);
                    } else {
                        state.record_usage(UsageEvent::ProviderFailure);
                    }
                    state.notify_error(format!("Translation failed: {error}"));
                }
                Err(error) => state.notify_error(format!("Translation task failed: {error}")),
            }
        });
    }

    /// Prepare one Review: bind the frozen document request, resolve the
    /// configured endpoint and credential, and show the exact disclosure.
    /// Nothing leaves the machine until [`send_review`](Self::send_review).
    pub fn start_review(&self) {
        self.prepare_review(false);
    }

    /// Prepare one Review of exactly the active source editor's selection:
    /// the request, the disclosure and the bytes are the selected text's
    /// alone. Without a non-empty selection nothing is prepared.
    pub fn start_review_selection(&self) {
        self.prepare_review(true);
    }

    /// Record the source editor `ui::document` mounted for `tab`: the one
    /// whose live selection `Review selection` reviews.
    pub fn attach_source_editor(&self, tab: u64, editor: ComponentRef<Editor>) {
        self.source_editor.set(Some((tab, editor)));
    }

    pub fn attach_document_view(&self, tab: u64, view: ElementRef) {
        self.document_view.set(Some((tab, view)));
    }

    pub fn attach_guard_view(&self, view: ElementRef) {
        self.guard_view.set(Some(view));
    }

    pub fn attach_compare_view(&self, view: ElementRef) {
        self.compare_view.set(Some(view));
    }

    pub fn attach_palette_view(&self, view: ElementRef) {
        self.palette_view.set(Some(view));
    }

    /// The topmost open layer, else the active pane's editor or document
    /// view; the pane never switches. The caret never moves.
    pub fn focus_editor(&self) {
        let Some(active) = self.active.get() else {
            return;
        };
        if self.focus_open_layer() {
            return;
        }
        if self
            .active_tab()
            .is_some_and(|tab| tab.pane.get().mounts_source())
        {
            // The new tab's editor mounts in a later commit: focus it when
            // it is there, never the container it replaced.
            self.retry_editor_focus(active, Self::FOCUS_RETRY_FRAMES);
        } else {
            // The Preview and Web panes mount their document the same way.
            self.retry_document_focus(active, Self::FOCUS_RETRY_FRAMES);
        }
    }

    /// Frames a pending editor focus waits for a tab switch to mount it.
    const FOCUS_RETRY_FRAMES: u8 = 6;

    fn retry_editor_focus(&self, active: u64, frames_left: u8) {
        if self.focus_mounted_editor(active) {
            return;
        }
        if frames_left == 0 {
            self.focus_document_view();
            return;
        }
        let state = self.clone();
        // The retry belongs to the app, not to the handler that asked for
        // the focus: a key binding in a card that this action closes would
        // take the pending frame down with it.
        self.owner.run(|| {
            request_animation_frame(move |_| {
                if state.active.get() != Some(active) || state.focus_open_layer() {
                    return;
                }
                state.retry_editor_focus(active, frames_left - 1);
            });
        });
    }

    fn focus_open_layer(&self) -> bool {
        if self.guard.get().is_some() {
            if let Some(view) = self.guard_view.get() {
                view.focus();
                #[cfg(test)]
                self.focus_last.set(Some(FocusTarget::Guard));
            }
            return true;
        }
        if self.review_consent.get().is_some() || self.revision_consent.get().is_some() {
            return true;
        }
        if self.palette_open.get() {
            if let Some(view) = self.palette_view.get() {
                view.focus();
                #[cfg(test)]
                self.focus_last.set(Some(FocusTarget::Palette));
            }
            return true;
        }
        if self.settings_open.get() {
            return true;
        }
        if crate::ui::notices::compare_is_open(self) {
            if let Some(view) = self.compare_view.get() {
                view.focus();
                #[cfg(test)]
                self.focus_last.set(Some(FocusTarget::Compare));
            }
            return true;
        }
        false
    }

    fn retry_document_focus(&self, active: u64, frames_left: u8) {
        if self.focus_mounted_document(active) {
            return;
        }
        if frames_left == 0 {
            return;
        }
        let state = self.clone();
        self.owner.run(|| {
            request_animation_frame(move |_| {
                if state.active.get() != Some(active) || state.focus_open_layer() {
                    return;
                }
                state.retry_document_focus(active, frames_left - 1);
            });
        });
    }

    fn focus_document_view(&self) -> bool {
        let Some(active) = self.active.get_untracked() else {
            return false;
        };
        self.focus_mounted_document(active)
    }

    /// Focus the document view only when it belongs to `active`: the
    /// subtree is keyed by tab, so the ref a switch replaces must not take
    /// the keyboard.
    fn focus_mounted_document(&self, active: u64) -> bool {
        let Some((_, view)) = self.document_view.get().filter(|(tab, _)| *tab == active) else {
            return false;
        };
        view.focus();
        #[cfg(test)]
        self.focus_last.set(Some(FocusTarget::Document(active)));
        true
    }

    fn focus_mounted_editor(&self, active: u64) -> bool {
        let Some(api) = self
            .source_editor
            .get()
            .filter(|(tab, _)| *tab == active)
            .and_then(|(_, editor)| editor.api::<EditorApi>())
        else {
            return false;
        };
        api.focus();
        #[cfg(test)]
        self.focus_last.set(Some(FocusTarget::Editor));
        true
    }

    /// Take the keyboard focus from the mounted source editor, if it has it:
    /// a card that floats over the room (consent, the close guard) owns
    /// Enter, Escape and Tab, which the focused editor would otherwise take
    /// first (its keys are the newest bindings while it is focused).
    pub fn blur_source_editor(&self) {
        if let Some(api) = self
            .source_editor
            .get()
            .and_then(|(_, editor)| editor.api::<EditorApi>())
        {
            api.blur();
        }
    }

    /// The non-empty primary selection the mounted source editor of `tab`
    /// shows now, as byte offsets into `text`. No editor on screen for the
    /// tab (another pane, another tab), or an editor that does not hold
    /// `text`, selects nothing.
    fn selection_in(&self, tab: &Tab, text: &str) -> Option<(usize, usize)> {
        let (owner, editor) = self.source_editor.get()?;
        let api = editor.api::<EditorApi>().filter(|_| owner == tab.id)?;
        // The editor holds the text with its line breaks as `\n`.
        if api.value() != text.replace("\r\n", "\n").replace('\r', "\n") {
            return None;
        }
        let primary = api.selections().pop()?;
        SourceSelection {
            anchor: (primary.anchor.line, primary.anchor.column),
            head: (primary.head.line, primary.head.column),
        }
        .byte_range(text)
    }

    /// [`start_review`](Self::start_review) for the whole document, or for
    /// the selection with `selection`.
    fn prepare_review(&self, selection: bool) {
        // A review that is prepared replaces any review still in flight: its
        // task writes nothing when it comes back, and the loading state it
        // left is not this run's to show — the early returns below must not
        // leave the field saying `Reviewing…`.
        self.review_generation.update(|generation| *generation += 1);
        self.review_busy.set(false);
        self.review_in_flight.borrow_mut().take();
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
        self.review_selection.set(None);
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
                self.notify_review_error(format!("Review unavailable: {error}"));
                return;
            }
        };
        let lens = ArtifactLens::infer_from_path(&tab.path);
        if selection && lens == ArtifactLens::AgentSkill {
            self.notify_held(SharedString::from(
                "An Agent Skill is reviewed as one package: use Review.",
            ));
            return;
        }
        let text = tab.text.get().to_string();
        let captured = self.selection_in(&tab, &text);
        if selection && captured.is_none() {
            self.notify_held(SharedString::from(
                "Select text in the source to review a selection.",
            ));
            return;
        }
        let range = captured.map(|(start, end)| start..end);
        let built = build_review_request(
            ReviewRequestBuildRequest::new(
                if selection {
                    ReviewTarget::Selection
                } else {
                    ReviewTarget::Document
                },
                lens,
                Some(&tab.path),
                &text,
                range.as_ref(),
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
                self.notify_review_error(format!("Review request rejected: {error}"));
                return;
            }
        };
        // A selection leaves alone: Effective Agent Context belongs to the
        // whole artifact.
        if let Some(context) = self.context_resolved.get().filter(|_| !selection) {
            let selected = self.context_selected.get();
            if !selected.is_empty() && context.input.target == tab.path {
                let selected = match context.select_sources(&selected) {
                    Ok(selected) => selected,
                    Err(error) => {
                        self.notify_review_error(format!("Context selection rejected: {error}"));
                        return;
                    }
                };
                request = match request.with_effective_agent_context(selected) {
                    Ok(request) => request,
                    Err(error) => {
                        self.notify_review_error(format!("Context request rejected: {error}"));
                        return;
                    }
                };
            }
        }
        let context_included = request.effective_agent_context.is_some();
        let bound = match prepared.bind_document_request(request, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.notify_review_error(format!("Review request rejected: {error}"));
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
        // The selection is offered only where a selection can be bound: an
        // Agent Skill package is reviewed whole.
        let selectable = matches!(bound.request().source, ReviewSource::Document { .. });
        *self.pending_review.borrow_mut() = Some(bound);
        *self.review_origin.borrow_mut() = Some(origin);
        self.review_selection.set(captured.filter(|_| selectable));
        self.review_consent
            .set(Some(SharedString::from(disclosure)));
        self.review_manifest.set(Some(manifest));
        self.review_open.set(true);
        self.sync_undo_surface();
    }

    /// Whether the waiting Review consent may be bound to the source
    /// selection captured when it was prepared (the `Selection` scope).
    pub fn review_consent_offers_selection(&self) -> bool {
        self.review_selection.get().is_some() && self.review_consent_target().is_some()
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
    /// the same frozen request over the captured selection, the whole
    /// document, or the document with the selected Effective Agent Context,
    /// and disclose the new scope. Nothing is authorized; the next send needs
    /// a fresh hold.
    pub fn set_review_consent_scope(&self, choice: ReviewScopeChoice) {
        let Some((target, base, language)) = self.review_base() else {
            return;
        };
        let scope = match choice {
            ReviewScopeChoice::Selection => {
                let Some((start, end)) = self.review_selection.get() else {
                    return;
                };
                match ByteRange::new(start as u64, end as u64) {
                    Ok(range) => ReviewScope::selection(range),
                    Err(error) => {
                        self.notify_review_error(format!("Review request rejected: {error}"));
                        return;
                    }
                }
            }
            ReviewScopeChoice::Document | ReviewScopeChoice::DocumentWithContext => {
                match base.scope {
                    ReviewScope::Selection { .. } => ReviewScope::Document,
                    whole => whole,
                }
            }
        };
        let mut request = match ReviewRequest::new(base.lens, scope, base.source, base.snapshot) {
            Ok(request) => request,
            Err(error) => {
                self.notify_review_error(format!("Review request rejected: {error}"));
                return;
            }
        };
        if choice == ReviewScopeChoice::DocumentWithContext {
            let context = match self.context_selection(&target) {
                Ok(Some(context)) => context,
                Ok(None) => return,
                Err(error) => {
                    self.notify_review_error(format!("Context selection rejected: {error}"));
                    return;
                }
            };
            request = match request.with_effective_agent_context(context) {
                Ok(request) => request,
                Err(error) => {
                    self.notify_review_error(format!("Context request rejected: {error}"));
                    return;
                }
            };
        }
        let settings = self.settings.get();
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_review_failure(&error);
                self.notify_review_error(format!("Review unavailable: {error}"));
                self.cancel_review();
                return;
            }
        };
        let context_included = request.effective_agent_context.is_some();
        let bound = match prepared.bind_document_request(request, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.notify_review_error(format!("Review request rejected: {error}"));
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
                    origin: if scope.kind() == OutboundScopeKind::Selection {
                        OutboundOrigin::Selection
                    } else {
                        OutboundOrigin::Document
                    },
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
        let partial = revision_request
            .source
            .package()
            .is_some_and(|package| package.is_partial());
        let revision_language = bound.language();
        let disclosure = bound.disclosure().clone();
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = match bound.authorize(&mut consent) {
            Ok(authorization) => authorization,
            Err(error) => {
                self.notify_review_error(format!("Review consent rejected: {error}"));
                return;
            }
        };

        self.review_consent.set(None);
        self.review_manifest.set(None);
        self.review_busy.set(true);
        *self.review_in_flight.borrow_mut() = Some(origin);
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
            *state.review_in_flight.borrow_mut() = None;
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
                    view.partial = partial;
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
            revision: origin.snapshot.revision,
            request,
            output,
            language,
        });
        self.cancel_revision();
        *self.revision_ledger.borrow_mut() = None;
        self.revision_not_applied.set(0);
        self.revision_questions.set(questions);
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
    }

    fn reset_revision(&self) {
        *self.revision_context.borrow_mut() = None;
        self.cancel_revision();
        *self.revision_ledger.borrow_mut() = None;
        self.revision_not_applied.set(0);
        self.revision_questions.set(Vec::new());
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
    }

    /// Build a Revision request from the frozen Review output and the answer
    /// states shown in the panel, then disclose it before any send. Nothing
    /// happens once the text changed since the Review: the Review describes
    /// text that is gone, and Review again is the only next step.
    pub fn start_revision(&self) {
        if self.revision_busy.get_untracked() {
            return;
        }
        let context = self.revision_context.borrow().as_ref().map(|context| {
            (
                context.tab_id,
                context.revision,
                context.request.clone(),
                context.output.clone(),
                context.language,
            )
        });
        let Some((tab_id, reviewed, request, output, language)) = context else {
            self.set_revision_status(REVISION_UNAVAILABLE, true);
            return;
        };
        let current = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.revision.get());
        if current != Some(reviewed) {
            return;
        }
        self.revision_not_applied.set(0);
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
                self.set_revision_status(format!("Answers rejected: {error}"), true);
                return;
            }
        };
        let settings = self.settings.get();
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_review_failure(&error);
                self.set_revision_status(format!("Revision unavailable: {error}"), true);
                return;
            }
        };
        let bound = match prepared.bind_revision_request(request, output, answers, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.set_revision_status(format!("Revision request rejected: {error}"), true);
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
        self.revision_generation
            .set(self.revision_generation.get().wrapping_add(1));
        if let Some(cancelled) = self.revision_cancellation.borrow_mut().take() {
            cancelled.store(true, Ordering::Release);
            if self.status_surface.get_untracked() == StatusSurface::Review {
                self.status.set(StatusMessage::default());
            }
        }
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
        // The answer belongs to the document this Review read, and only for
        // as long as this context lives.
        let Some(tab_id) = self
            .revision_context
            .borrow()
            .as_ref()
            .map(|context| context.tab_id)
        else {
            self.set_revision_status(REVISION_UNAVAILABLE, true);
            return;
        };
        let generation = self.revision_generation.get();
        let disclosure = bound.disclosure().clone();
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = match bound.authorize(&mut consent) {
            Ok(authorization) => authorization,
            Err(error) => {
                self.set_revision_status(format!("Revision consent rejected: {error}"), true);
                return;
            }
        };
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_busy.set(true);
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.revision_cancellation.borrow_mut() = Some(Arc::clone(&cancelled));
        for answer in bound.answers().as_slice() {
            if matches!(answer, RevisionAnswer::Answered(_)) {
                self.record_usage(UsageEvent::ClarificationAnswered);
            }
        }
        let state = self.clone();
        self.spawn_task(async move {
            let result = blocking(move || {
                bound
                    .execute_with(authorization, &cancelled, REVISION_REQUEST_TIMEOUT)
                    .map_err(RevisionExecutionError::into_revision_error)
            })
            .await;
            if state.revision_generation.get() != generation {
                return;
            }
            state.revision_cancellation.borrow_mut().take();
            state.revision_busy.set(false);
            match result {
                Ok(Ok(transport)) => state.present_revision_for(tab_id, transport.into_proposal()),
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
                    state.set_revision_status(format!("Revision failed: {error}"), true);
                }
                Err(error) => {
                    state.set_revision_status(format!("Revision task failed: {error}"), true)
                }
            }
        });
    }

    /// Present `proposal` over the tab on screen, as if a Review of it had
    /// prepared the proposal: the unit tests' way in, with no provider.
    #[cfg(test)]
    pub(crate) fn present_revision(&self, proposal: RevisionProposal) {
        if let Some(tab_id) = self.active.get() {
            self.present_revision_for(tab_id, proposal);
        }
    }

    /// Show a validated proposal for per-change decisions: its hunks in the
    /// field, and a ledger placing them in the text of `tab_id`, the tab the
    /// Review read. Nothing is written until the user accepts a change.
    fn present_revision_for(&self, tab_id: u64, proposal: RevisionProposal) {
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
        *self.revision_ledger.borrow_mut() = Some(RevisionLedger::new(tab_id, &proposal));
        self.revision_not_applied.set(0);
        self.revision_hunks.set(hunks);
        self.set_revision_status(REVISION_READY, false);
    }

    fn set_revision_status(&self, message: impl Into<SharedString>, error: bool) {
        self.revision_status_error.set(error);
        self.revision_status.set(Some(message.into()));
    }

    /// Write one change into the buffer now, as one edit: `applied` puts its
    /// replacement in place of the reviewed bytes, otherwise the reviewed
    /// bytes go back exactly. Returns whether the buffer now reads as asked;
    /// a refusal says why on the status line and changes nothing. Nothing is
    /// saved — the tab is only dirty.
    pub fn set_revision_change_applied(&self, change_id: u32, applied: bool) -> bool {
        let change = ChangeId(change_id);
        let mut slot = self.revision_ledger.borrow_mut();
        let Some(ledger) = slot.as_mut() else {
            drop(slot);
            self.set_revision_status(REVISION_UNAVAILABLE, true);
            return false;
        };
        if ledger.applied(change) == applied {
            return true;
        }
        let Some(tab) = self.active_tab().filter(|tab| tab.id == ledger.tab_id()) else {
            drop(slot);
            self.record_usage(UsageEvent::StaleResultBlocked);
            self.set_revision_status(
                "This Revision belongs to another document. Return to it to decide.",
                false,
            );
            return false;
        };
        let written = match tab
            .text
            .with(|text| ledger.set_applied(change, applied, text))
        {
            Ok(written) => written,
            Err(refusal) => {
                drop(slot);
                let message = match (refusal, applied) {
                    (LedgerRefusal::UnknownChange, _) => "This change is not part of the Revision.",
                    (LedgerRefusal::PartChanged, true) => {
                        self.record_usage(UsageEvent::StaleResultBlocked);
                        "Can't apply — this part changed."
                    }
                    (LedgerRefusal::PartChanged, false) => {
                        "Can't undo — this part was edited after it was accepted."
                    }
                };
                self.set_revision_status(message, true);
                return false;
            }
        };
        // The ledger already follows the new text: `note_edit` below finds
        // nothing left to observe.
        drop(slot);
        if let Some(hunk) = self
            .revision_hunks
            .get()
            .iter()
            .find(|hunk| hunk.change_id == change_id)
        {
            // Every hunk of one change shares this signal.
            hunk.accepted.set(applied);
        }
        if let Some(text) = written {
            self.write_model_text(&tab, text);
        }
        self.set_revision_status(REVISION_READY, false);
        true
    }

    /// Flip the hunk's change between accepted (written) and not.
    pub fn toggle_revision_hunk(&self, index: usize) {
        if let Some(hunk) = self.revision_hunks.get().get(index) {
            self.set_revision_change_applied(hunk.change_id, !hunk.accepted.get());
        }
    }

    /// Whether the hunk at `index` no longer finds its own text where it
    /// left it — an edit touched the part it covers — so it can neither be
    /// accepted nor, once accepted, taken back. Reads the tab's text, so a
    /// binding follows every edit.
    pub fn revision_hunk_part_changed(&self, index: usize) -> bool {
        let ledger = self.revision_ledger.borrow();
        let Some(ledger) = ledger.as_ref() else {
            return true;
        };
        let tab = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == ledger.tab_id())
            .cloned();
        tab.is_none_or(|tab| tab.text.with(|text| ledger.part_changed(index, text)))
    }

    /// Where the hunk at `index` sits in the text of tab `tab_id` now: the
    /// bytes of its current side, as the ledger places them. `None` when no
    /// proposal is live, when it belongs to another tab, or once an edit
    /// touched the hunk. Reads the tab's text, so a binding follows every
    /// edit.
    pub fn revision_hunk_range(&self, tab_id: u64, index: usize) -> Option<std::ops::Range<usize>> {
        let ledger = self.revision_ledger.borrow();
        let ledger = ledger.as_ref().filter(|ledger| ledger.tab_id() == tab_id)?;
        let tab = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()?;
        tab.text.with(|text| ledger.current_range(index, text))
    }

    /// The approved result of the live proposal: the text of the tab it
    /// belongs to, as it stands — every accepted change is already in it,
    /// nothing else of the proposal is. `None` when no proposal is live.
    pub fn revision_approved_text(&self) -> Option<SharedString> {
        let tab_id = self.revision_ledger.borrow().as_ref()?.tab_id();
        self.tabs
            .get_untracked()
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.text.get_untracked())
    }

    /// Follow an edit of tab `id` with the live proposal's ledger, so each
    /// hunk the edit did not touch keeps its place.
    fn observe_revision_edit(&self, id: u64) {
        let mut slot = self.revision_ledger.borrow_mut();
        let Some(ledger) = slot.as_mut().filter(|ledger| ledger.tab_id() == id) else {
            return;
        };
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id) {
            tab.text.with(|text| ledger.observe(text));
        }
    }

    /// `Done`: the decisions stand as written — every accepted change is
    /// already in the buffer — and the `undecided` hunks are discarded,
    /// never written; the outcome line counts them. Nothing is saved.
    pub fn finish_revision(&self, undecided: usize) {
        if self.revision_ledger.borrow_mut().take().is_none() {
            self.set_revision_status(REVISION_UNAVAILABLE, true);
            return;
        }
        let hunks = self.revision_hunks.get();
        let accepted_hunks = hunks.iter().filter(|hunk| hunk.accepted.get()).count() as u64;
        self.record_usage(UsageEvent::RevisionDecision {
            accepted_hunks,
            rejected_hunks: hunks.len() as u64 - accepted_hunks,
        });
        if accepted_hunks > 0 {
            self.record_usage(UsageEvent::RevisionApplied);
        }
        self.revision_hunks.set(Vec::new());
        self.revision_not_applied.set(undecided);
        self.set_revision_status(
            if accepted_hunks > 0 {
                "Approved changes applied to the buffer."
            } else {
                "No changes approved."
            },
            false,
        );
    }

    /// Whether the live proposal belongs to a tab other than the one on
    /// screen: its hunks are placed in that tab's text, so nothing can be
    /// decided into this one. An edit to the proposal's own tab does not
    /// make the whole proposal stale — each hunk the edit touched says so on
    /// its own ([`revision_hunk_part_changed`](Self::revision_hunk_part_changed)).
    pub fn revision_is_stale(&self) -> bool {
        let ledger = self.revision_ledger.borrow();
        ledger
            .as_ref()
            .is_some_and(|ledger| self.active.get() != Some(ledger.tab_id()))
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
        self.review_in_flight.borrow_mut().take();
        self.cancel_review();
    }

    pub fn close_review(&self) {
        self.cancel_review();
        self.reset_revision();
        self.review_view.set(None);
        self.review_unavailable.set(None);
        self.review_open.set(false);
        self.sync_undo_surface();
    }

    pub fn open_settings(&self) {
        self.refresh_usage_counts();
        self.settings_open.set(true);
        self.sync_undo_surface();
    }

    pub fn close_settings(&self) {
        if self.persist_settings(StatusSurface::Editor) {
            self.settings_open.set(false);
            self.sync_undo_surface();
        }
    }

    /// Escape closes at most one layer, in the contract's fixed order — the
    /// title menu, the guard or consent card, then the palette. Every Escape
    /// binding runs this, so which layer closes never depends on which
    /// binding the keymap offered the key to. Returns whether one closed.
    pub fn escape_top_layer(&self) -> bool {
        if self.title_menu_open.get() {
            self.close_title_menu();
            return true;
        }
        if self.guard.get().is_some() {
            self.guard_cancel();
            return true;
        }
        if self.review_consent.get().is_some() {
            self.cancel_review();
            self.focus_editor();
            return true;
        }
        if self.revision_consent.get().is_some() {
            crate::ui::review::cancel_revision(self);
            self.focus_editor();
            return true;
        }
        if self.palette_open.get() {
            self.close_palette();
            return true;
        }
        false
    }

    pub fn close_title_menu(&self) {
        self.title_menu_open.set(false);
        if self.active.get().is_some() {
            self.focus_editor();
        }
    }

    pub fn close_palette(&self) {
        self.palette_open.set(false);
        self.clear_search();
        if self.active.get().is_some() {
            self.focus_editor();
        }
    }

    pub fn save_settings(&self) {
        self.persist_settings(StatusSurface::Settings);
    }

    fn persist_settings(&self, success_surface: StatusSurface) -> bool {
        use crate::i18n::{Key, t};
        let settings = self.settings.get();
        let (key, kind, saved) = match settings.try_save_general() {
            Ok(_) => (Key::SettingsSaved, StatusKind::Success, true),
            Err(error) => {
                log::warn!("cannot save settings: {}", error.kind());
                (Key::SettingsSaveFailed, StatusKind::Error, false)
            }
        };
        let surface = if saved {
            success_surface
        } else {
            StatusSurface::Settings
        };
        self.notify_at(surface, kind, t(settings.language, key));
        saved
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
    ///
    /// A store that could not be read is never remembered (the caller may be
    /// looking at a moment of contention), and
    /// [`forget_credential_status`](Self::forget_credential_status) drops the
    /// memo for a caller that knows the store may have changed under it.
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
        // A failure is a moment, not a state: retry it on the next read rather
        // than pin it for the life of the process.
        if status != CredentialStatus::Unavailable {
            self.settings_credential_status.set(Some((key, status)));
        }
        status
    }

    /// Drop the memoised credential status, so the next read resolves the vault
    /// again. The settings page calls this whenever it is built: the secure
    /// store can change under it (another instance storing or removing a key),
    /// and nothing in this process reports that.
    pub(crate) fn forget_credential_status(&self) {
        self.settings_credential_status.set(None);
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

fn untitled_number(title: &str) -> Option<u64> {
    title.rsplit(' ').next()?.parse().ok()
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

/// The state's copy. New strings live here rather than in `i18n.rs` (a merge
/// hotspot); the `match` is exhaustive, so a missing translation is a compile
/// error.
mod s {
    use mt_core::{document::io::ConcurrentCommitOutcome, settings::Language};

    use super::SaveError;

    pub(super) fn opening_folder(language: Language) -> &'static str {
        match language {
            Language::English => "Opening folder…",
            Language::Chinese => "正在打开文件夹…",
        }
    }

    pub(super) fn search_needs_folder(language: Language) -> &'static str {
        match language {
            Language::English => "Open a folder first to search its files.",
            Language::Chinese => "请先打开文件夹以搜索其中的文件。",
        }
    }

    pub(super) fn untitled(language: Language, number: u64) -> String {
        match language {
            Language::English => format!("Untitled {number}"),
            Language::Chinese => format!("未命名 {number}"),
        }
    }

    pub(super) fn discarded_draft(language: Language) -> &'static str {
        match language {
            Language::English => "Discarded recovered draft",
            Language::Chinese => "已丢弃恢复的草稿",
        }
    }

    pub(super) fn undo(language: Language) -> &'static str {
        match language {
            Language::English => "Undo",
            Language::Chinese => "撤销",
        }
    }

    pub(super) fn save_cancelled(language: Language) -> &'static str {
        match language {
            Language::English => "Save cancelled — the document stays open.",
            Language::Chinese => "已取消保存——文档保持打开。",
        }
    }

    /// `guard_save_failed` with the write's own reason embedded, in the
    /// contract's `{error}` position (contract amendment 10). The app's own
    /// refusals are localized; an OS I/O error keeps the OS's wording.
    pub(super) fn save_as_write_failed(language: Language, error: &SaveError) -> String {
        let reason = save_refusal(language, error);
        match language {
            Language::English => format!(
                "Could not save this document ({reason}), so it stays open. Check file permissions or try Save As…."
            ),
            Language::Chinese => {
                format!("文档未能保存（{reason}），已保持打开。请检查文件权限或尝试“另存为…”。")
            }
        }
    }

    /// Why the write failed, in the user's language. Only the OS's own I/O
    /// text is passed through unchanged (contract amendment 10): a wrapped
    /// library error keeps neither its English nor the path it names.
    fn save_refusal(language: Language, error: &SaveError) -> String {
        if matches!(language, Language::English) {
            return error.to_string();
        }
        match error {
            SaveError::Io(error) => match error.raw_os_error() {
                Some(code) => std::io::Error::from_raw_os_error(code).to_string(),
                None => "写入失败".to_owned(),
            },
            SaveError::Conflict => {
                "文件在打开后已在磁盘上更改。请比较版本、保留你的版本、采用磁盘版本，或选择“另存为…”。"
                    .to_owned()
            }
            SaveError::DestinationExists => {
                "目标位置已存在同名文件；请选择是否替换。".to_owned()
            }
            SaveError::Missing => "源文件路径已不存在；请重新创建或另存副本。".to_owned(),
            SaveError::SourceIdentityChanged => {
                "源文件路径或其符号链接目标已更改；请另存副本。".to_owned()
            }
            SaveError::DecodeLoss => {
                "原始字节无法精确解码；请转换为 UTF-8 或另存副本。".to_owned()
            }
            SaveError::Unrepresentable { encoding } => {
                format!("文本无法以 {encoding} 表示；请转换为 UTF-8 或另存副本。")
            }
            SaveError::ConcurrentCommit { outcome, .. } => match outcome {
                ConcurrentCommitOutcome::ExternalVersionRestored => {
                    "并发写入已恢复到原始目标位置；请检查保留的副本并另存。".to_owned()
                }
                ConcurrentCommitOutcome::Indeterminate => {
                    "并发写入导致保存结果不确定；请检查保留的文件并另存。".to_owned()
                }
            },
        }
    }

    pub(super) fn guard_save_failed(language: Language) -> &'static str {
        match language {
            Language::English => {
                "Could not save this document, so it stays open. Check file permissions or try Save As…."
            }
            Language::Chinese => "文档未能保存，已保持打开。请检查文件权限或尝试“另存为…”。",
        }
    }

    pub(super) fn quit_save_failed(language: Language) -> &'static str {
        match language {
            Language::English => {
                "Quit cancelled: a document could not be saved. Check the open tabs and try again."
            }
            Language::Chinese => "已取消退出：有文档未能保存。请检查打开的标签页后重试。",
        }
    }

    pub(super) fn recovered_conflict(language: Language) -> &'static str {
        match language {
            Language::English => {
                "Recovered draft differs from the file on disk. Compare versions, keep yours, use the disk version, or choose Save As…."
            }
            Language::Chinese => {
                "恢复的草稿与磁盘文件不同。请比较版本、保留你的版本、采用磁盘版本，或选择“另存为…”。"
            }
        }
    }

    pub(super) fn untitled_save_first(language: Language) -> &'static str {
        match language {
            Language::English => "This draft has no file yet. Choose Save As… to save it.",
            Language::Chinese => "此草稿尚未关联文件，请使用“另存为…”保存。",
        }
    }

    pub(super) fn sample_unavailable(language: Language, error: &dyn std::fmt::Display) -> String {
        match language {
            Language::English => format!("Could not open the bundled sample: {error}."),
            Language::Chinese => format!("无法打开内置示例：{error}。"),
        }
    }

    pub(super) fn save_as_picker_failed(
        language: Language,
        error: &dyn std::fmt::Display,
    ) -> String {
        match language {
            Language::English => format!("Could not open the Save As dialog: {error}. Try again."),
            Language::Chinese => format!("无法打开“另存为”对话框：{error}。请重试。"),
        }
    }

    pub(super) fn checkpoint_failed(language: Language, error: &dyn std::fmt::Display) -> String {
        match language {
            Language::English => format!(
                "Automatic recovery backup failed ({error}). Your edits are still in this window; save the file to disk soon."
            ),
            Language::Chinese => {
                format!("自动恢复备份失败（{error}）。你的编辑仍在此窗口中，请尽快保存文件。")
            }
        }
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

/// What the last local focus move handed the keyboard to, for the lib tests:
/// one per app, so parallel tests cannot see each other's focus.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusTarget {
    Editor,
    Document(u64),
    Guard,
    Palette,
    Compare,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mt_core::{
        credentials::{CredentialError, Secret, SecureCredentialStore},
        review::{
            ByteRange,
            revision::{RevisionChange, RevisionEdit, RevisionLimits},
        },
    };

    use super::*;

    /// A credential store with nothing in it: the test never reaches the
    /// platform's secure store.
    struct NoStore;

    impl SecureCredentialStore for NoStore {
        fn is_supported(&self) -> bool {
            false
        }
        fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
            Ok(None)
        }
        fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    const SOURCE: &str = "compiles but no check\n";

    type Outcome = (String, String);

    static OUTCOME: std::sync::Mutex<Option<Outcome>> = std::sync::Mutex::new(None);

    fn read_text(tab: &Tab) -> String {
        tab.text.get().to_string()
    }

    #[component]
    fn RevisionWriteProbe() -> impl IntoElement {
        let (state, tab) = proposed();
        assert!(state.set_revision_change_applied(0, true), "accept");
        let accepted = read_text(&tab);
        assert!(state.set_revision_change_applied(0, false), "undo");
        let undone = read_text(&tab);
        *OUTCOME.lock().expect("the outcome") = Some((accepted, undone));
        view()
    }

    /// A new document holding [`SOURCE`] with a one-change Revision on
    /// screen, over an empty credential store.
    fn proposed() -> (AppState, Tab) {
        let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
        state.new_document();
        let tab = state.active_tab().expect("the new document");
        tab.text.set(SharedString::from(SOURCE));
        let changes = [("Tighten.", "compiles but", "compiles, but")]
            .iter()
            .map(|&(reason, before, after)| {
                let start = SOURCE.find(before).expect("in the source") as u64;
                let range = ByteRange::new(start, start + before.len() as u64).expect("a range");
                RevisionChange::new(reason, vec![RevisionEdit::new(range, before, after)])
            })
            .collect();
        let proposal = RevisionProposal::validate(
            SOURCE,
            SourceSnapshot::new(0, 0),
            changes,
            RevisionLimits::default(),
        )
        .expect("a valid proposal");
        state.present_revision(proposal);
        (state, tab)
    }

    #[test]
    fn an_error_routes_to_the_settings_status_while_it_is_open() {
        #[component]
        fn ErrorRoutingProbe() -> impl IntoElement {
            let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
            state.notify_error("checkpoint failed");
            let editor = state.status_surface.get();
            state.settings_open.set(true);
            state.notify_error("checkpoint failed");
            let settings = state.status_surface.get();
            let routed = editor == StatusSurface::Editor && settings == StatusSurface::Settings;
            view().child(text(if routed { "routed" } else { "wrong surface" }))
        }
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                ErrorRoutingProbe,
            >()))
            .expect("start the probe");
        app.wait_for("text:routed")
            .expect("the settings surface receives the error");
    }

    static OPENED_FOLDER: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

    #[component]
    fn OpenFolderProbe() -> impl IntoElement {
        let directory = tempfile::tempdir().expect("the folder fixture");
        std::fs::write(directory.path().join("plan.md"), "# Plan\n").expect("write the file");
        let path = directory.keep();
        let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
        state.open_folder(path);
        let opening = state.status.get_untracked().text.to_string();
        let watched = state.clone();
        effect(move || {
            if watched.root.get().is_some() && watched.entries.get().len() == 1 {
                *OPENED_FOLDER.lock().expect("the outcome") = Some((opening.clone(), true));
            }
        });
        view().child(text(move || {
            if watched.root.get().is_some() {
                "folder open"
            } else {
                "opening"
            }
        }))
    }

    #[test]
    fn opening_a_folder_says_so_at_once_and_lists_it_after() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                OpenFolderProbe,
            >()))
            .expect("start the probe");
        app.wait_for("text:folder open")
            .expect("the walk lands after the status");
        let (opening, opened) = OPENED_FOLDER
            .lock()
            .expect("the outcome")
            .take()
            .expect("the probe ran");
        assert_eq!(opening, "Opening folder\u{2026}");
        assert!(opened, "the entries arrive after the walk");
    }

    #[test]
    fn revision_accept_and_undo_update_the_buffer() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                RevisionWriteProbe,
            >()))
            .expect("start the probe");
        app.first_render().expect("render the probe");
        let (accepted, undone) = OUTCOME
            .lock()
            .expect("the outcome")
            .take()
            .expect("the probe ran");
        assert_eq!(accepted, "compiles, but no check\n");
        assert_eq!(undone, SOURCE);
    }

    /// The status after a first and a second `Done` over one proposal.
    static DONE_TWICE: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

    #[component]
    fn DoneTwiceProbe() -> impl IntoElement {
        let (state, _tab) = proposed();
        let status = || {
            state
                .revision_status
                .get()
                .map(|status| status.to_string())
                .unwrap_or_default()
        };
        state.finish_revision(1);
        let first = status();
        state.finish_revision(1);
        *DONE_TWICE.lock().expect("the outcome") = Some((first, status()));
        view()
    }

    #[test]
    fn a_second_done_without_a_proposal_decides_nothing() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                DoneTwiceProbe,
            >()))
            .expect("start the probe");
        app.first_render().expect("render the probe");
        let (first, second) = DONE_TWICE
            .lock()
            .expect("the outcome")
            .take()
            .expect("the probe ran");
        assert_eq!(first, "No changes approved.");
        assert_eq!(second, REVISION_UNAVAILABLE);
    }

    /// Whether `start_revision` did anything (a disclosure or a status),
    /// over a Review of the text as it was, then over one the text has
    /// since moved past.
    static PREPARED: std::sync::Mutex<Option<(bool, bool)>> = std::sync::Mutex::new(None);

    #[component]
    fn StaleRevisionProbe() -> impl IntoElement {
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: ReviewScope::Document,
            understood_intent: mt_core::review::ReviewSections {
                stated_goal: "keep the plan".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "a plan".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: Vec::new(),
        };
        let prepared = |edit: bool| {
            let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
            state.new_document();
            let tab = state.active_tab().expect("the new document");
            tab.text.set(SharedString::from(SOURCE));
            let snapshot = SourceSnapshot::new(tab.revision.get(), 0);
            let request = ReviewRequest::document(ArtifactLens::default_lens(), SOURCE, snapshot)
                .expect("a review request");
            let origin = ReviewOrigin {
                tab_id: tab.id,
                snapshot,
            };
            state.set_revision_context(origin, request, output.clone(), ReviewLanguage::English);
            if edit {
                tab.revision.update(|revision| *revision += 1);
            }
            state.start_revision();
            state.revision_consent.get().is_some() || state.revision_status.get().is_some()
        };
        *PREPARED.lock().expect("the outcome") = Some((prepared(false), prepared(true)));
        view()
    }

    #[test]
    fn a_review_the_text_moved_past_prepares_no_revision() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                StaleRevisionProbe,
            >()))
            .expect("start the probe");
        app.first_render().expect("render the probe");
        let (current, stale) = PREPARED
            .lock()
            .expect("the outcome")
            .take()
            .expect("the probe ran");
        assert!(current, "a current review prepared nothing");
        assert!(!stale, "a stale review prepared a revision");
    }

    const UNSET_SOURCE: &str = "# Plan\n\nKeep this text.\n";

    /// The whole room over one clean document, on an endpoint no environment
    /// key is allowed for and an empty credential store: no model is set up,
    /// and nothing can reach the platform's secure store or the network.
    /// Beside the room, the document's exact text and dirty flag.
    fn not_set_up_room(language: Language) -> impl IntoElement {
        let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
        state.settings.update(|settings| {
            settings.language = language;
            settings.model_provider = mt_core::model::Provider::OpenAiResponses.key().to_owned();
            settings.model_base_url = "https://review.markturbo-test.invalid/v1/".to_owned();
            settings.model_environment_key_identity = String::new();
        });
        state.new_document();
        let tab = state.active_tab().expect("the new document");
        tab.text.set(SharedString::from(UNSET_SOURCE));
        let (source, dirty) = (tab.text, tab.dirty);
        view()
            .flex_1()
            .w_full()
            .h(relative(1.0))
            .child(text(move || {
                format!("Probe: {:?} dirty={}", source.get(), dirty.get())
            }))
            .child(crate::ui::shell(
                state,
                language,
                crate::ui::tokens::Layout::Wide,
            ))
    }

    #[component]
    fn NotSetUpProbe() -> impl IntoElement {
        not_set_up_room(Language::English)
    }

    #[component]
    fn NotSetUpChineseProbe() -> impl IntoElement {
        not_set_up_room(Language::Chinese)
    }

    /// `Ctrl+Enter` from the focused source editor with no model set up:
    /// the field says so, with the way to Settings, and the document's text
    /// and dirty flag are as they were. Returns the app with the diagnostic
    /// on screen.
    fn review_says_not_set_up<P: Component>(
        headline: &str,
        open_settings: &str,
    ) -> nsh_testing::Headless
    where
        P::Props: Default,
    {
        let mut engine = nsh_engine::EngineConfig::new(nsh_engine::Root::of::<P>());
        engine.environment.os = nsh_core::platform::Os::Windows;
        engine.environment.idiom = nsh_core::platform::Idiom::Desktop;
        let mut app = nsh_testing::Headless::start(engine).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(
            nsh_engine::Points(1440.0),
            nsh_engine::Points(900.0),
        ))
        .expect("viewport");
        let probe = format!("Probe: {UNSET_SOURCE:?} dirty=false");
        app.wait_for(&format!("text:{probe}"))
            .expect("the clean document");
        app.wait_for("label:Markdown source")
            .expect("the source editor");
        app.type_text("label:Markdown source", "")
            .expect("focus the editor");
        assert_eq!(
            app.screen()
                .count("id:review-unavailable")
                .expect("the selector"),
            0,
            "nothing says so before Review is asked for"
        );
        app.press_key("ctrl+enter").expect("press Ctrl+Enter");
        // Review prepares off the UI thread; under a loaded parallel suite
        // that can outlast the default wait, as in the other Review tests.
        app.wait_for_within("id:review-unavailable", std::time::Duration::from_secs(20))
            .expect("Review says why it cannot run");
        let screen = app.screen();
        for (what, copy) in [("headline", headline), ("way forward", open_settings)] {
            assert!(
                screen
                    .matches(&format!("id:review-unavailable >> text:{copy}"))
                    .expect("the selector"),
                "the {what} `{copy}`: {:?}",
                screen.texts()
            );
        }
        assert!(
            app.shows(&probe),
            "the text and dirty flag are unchanged: {:?}",
            app.screen().texts()
        );
        app
    }

    #[test]
    fn review_without_a_model_says_no_model_set_up() {
        review_says_not_set_up::<NotSetUpProbe>("No model set up", "Open settings");
    }

    #[test]
    fn review_without_a_model_says_so_in_chinese() {
        review_says_not_set_up::<NotSetUpChineseProbe>("未配置模型", "打开设置");
    }

    /// With no model set up, the field's Close takes the diagnostic away,
    /// and the title menu's Review row brings it back; neither touches the
    /// document's text or dirty flag.
    #[test]
    fn review_without_a_model_closes_and_the_menu_row_shows_it_again() {
        let mut app = review_says_not_set_up::<NotSetUpProbe>("No model set up", "Open settings");
        let probe = format!("Probe: {UNSET_SOURCE:?} dirty=false");

        app.press("id:review-close").expect("close the field");
        app.wait_for_gone("id:review-unavailable", std::time::Duration::from_secs(5))
            .expect("Close takes the diagnostic away");

        app.press("id:title-settings").expect("open the title menu");
        app.press("id:title-menu-review-selection")
            .expect("the menu's Review row");
        app.wait_for_within("id:review-unavailable", std::time::Duration::from_secs(20))
            .expect("the menu row says why Review cannot run");
        assert!(
            app.shows(&probe),
            "the text and dirty flag are unchanged: {:?}",
            app.screen().texts()
        );
    }

    static REPLACED: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

    #[component]
    fn ReplaceProbe() -> impl IntoElement {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("plan.md");
        std::fs::write(
            &path, "on disk
",
        )
        .expect("the file on disk");
        let state = AppState::with_vault(CredentialVault::with_test_store(Arc::new(NoStore)));
        state.new_document();
        let tab = state.active_tab().expect("the new document");
        tab.text.set(SharedString::from(SOURCE));
        state.write_model_text(
            &tab,
            "translated
"
            .to_owned(),
        );
        let translated = read_text(&tab);
        state.adopt_disk_version(&tab, io::load(&path).expect("load the file"));
        let reloaded = read_text(&tab);
        *REPLACED.lock().expect("the outcome") = Some((translated, reloaded));
        view()
    }

    #[test]
    fn a_translation_and_disk_reload_update_the_buffer() {
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                ReplaceProbe,
            >()))
            .expect("start the probe");
        app.first_render().expect("render the probe");
        let (translated, reloaded) = REPLACED
            .lock()
            .expect("the outcome")
            .take()
            .expect("the probe ran");
        assert_eq!(translated, "translated\n");
        assert_eq!(reloaded, "on disk\n");
    }

    /// The Save As guard's Chinese copy embeds the app's own refusal reason in
    /// Chinese; only an OS I/O error keeps the OS's wording (contract
    /// amendment 10).
    #[test]
    fn a_chinese_save_as_refusal_does_not_show_the_apps_english_reason() {
        let refusal = s::save_as_write_failed(Language::Chinese, &SaveError::Conflict);
        assert!(
            !refusal.contains("the file changed on disk"),
            "the app's English reason leaks into the Chinese guard: {refusal}"
        );
        assert!(
            refusal.contains("磁盘"),
            "the Chinese guard does not name the conflict in Chinese: {refusal}"
        );

        let os = SaveError::Io(std::io::Error::from_raw_os_error(5));
        let refusal = s::save_as_write_failed(Language::Chinese, &os);
        assert!(
            refusal.contains("os error 5"),
            "the OS's own error text must be kept: {refusal}"
        );

        let custom = SaveError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access is denied. (os error 5)",
        ));
        let refusal = s::save_as_write_failed(Language::Chinese, &custom);
        assert!(
            !refusal.contains("Access is denied"),
            "a library message wrapped in an io::Error leaks: {refusal}"
        );
        assert!(
            refusal.contains("写入失败"),
            "a wrapped error falls back to the localized reason: {refusal}"
        );

        let missing = tempfile::tempdir()
            .expect("temp dir")
            .path()
            .join("missing");
        let staged = tempfile::Builder::new()
            .prefix(".markturbo-")
            .tempfile_in(&missing)
            .expect_err("staging into a missing directory fails");
        let refusal = s::save_as_write_failed(Language::Chinese, &SaveError::Io(staged));
        assert!(
            refusal.contains("写入失败"),
            "the staged write falls back to the localized reason: {refusal}"
        );
        assert!(
            !refusal.contains("at path") && !refusal.contains(&missing.display().to_string()),
            "the staged write leaks the library's path wording: {refusal}"
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::SourceSelection;

    fn range(text: &str, anchor: (u32, u32), head: (u32, u32)) -> Option<(usize, usize)> {
        SourceSelection { anchor, head }.byte_range(text)
    }

    #[test]
    fn editor_positions_map_to_the_text_bytes_they_select() {
        let text = "ab\n\u{4e2d}\u{6587}\u{1f600}x\r\nz";
        let line = text.find('\u{4e2d}').unwrap();
        // UTF-16 columns: two CJK units and one surrogate pair before `x`.
        let x = text.find('x').unwrap();
        assert_eq!(range(text, (1, 0), (1, 4)), Some((line, x)));
        assert_eq!(&text[line..x], "\u{4e2d}\u{6587}\u{1f600}");
        // Selected backwards, the range is the same.
        assert_eq!(range(text, (1, 4), (1, 0)), Some((line, x)));
        // A column past the line stops before its `\r\n`.
        assert_eq!(range(text, (1, 0), (1, 99)), Some((line, x + 1)));
        // Across lines, and a line past the text clamps to its end.
        assert_eq!(range(text, (0, 1), (9, 0)), Some((1, text.len())));
        // A lone `\r` breaks a line, as it does in the editor.
        assert_eq!(range("a\rbc\r\nd", (1, 0), (1, 2)), Some((2, 4)));
        assert_eq!(range("a\rbc\r\nd", (2, 0), (2, 1)), Some((6, 7)));
        // A caret selects nothing.
        assert_eq!(range(text, (2, 1), (2, 1)), None);
        assert_eq!(range("", (0, 0), (3, 3)), None);
    }
}
