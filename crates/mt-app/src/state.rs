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
    time::{Duration, Instant},
};

use mt_core::{
    Document,
    agent_artifacts::{
        context::{CODEX_PROFILE_ID, ContextInput, ProjectTrust, ResolvedContext, resolve},
        instruction::{self, Instruction},
        package::{ReviewRequestBuildRequest, ReviewTarget, build_review_request},
        skill::{self, Discovery, Skill},
    },
    credentials::CredentialVault,
    document::io::{self, LoadedFile, SaveError},
    model::{ConsentCapability, ConsentDecision},
    recovery::{RecoveredRecord, RecoveryCheckpoint, RecoveryKey, RecoveryMetadata, RecoveryStore},
    review::{
        ArtifactLens, ClarificationPriority, FindingKind, ReviewModelOutput, ReviewRequest,
        ReviewResult, ReviewStatus, SourceSnapshot,
        provider::{
            PreparedDocumentReviewRequest, PreparedReview, PreparedRevisionRequest, ReviewError,
            ReviewLanguage, RevisionAnswer, RevisionAnswers,
        },
        revision::{ChangeId, RevisionProposal},
    },
    settings::{Language, RecentTarget, RecentTargetKind, SettingsData, ThemePreference},
    translate::{
        Scope, TranslationRequest,
        provider::{PreparedTranslation, PreparedTranslationRequest},
    },
    workspace::{
        search::{self, Query},
        tree::{self, FileNode},
    },
};
use nsh_base::alert_dialog;
use nsh_core::{file_dialog::SaveDialog, prelude::*};

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
    pub review_view: Signal<Option<ReviewView>>,
    pub review_busy: Signal<bool>,
    pub translation_busy: Signal<bool>,
    pub revision_questions: Signal<Vec<RevisionQuestionView>>,
    pub revision_consent: Signal<Option<SharedString>>,
    pub revision_hunks: Signal<Vec<RevisionHunkView>>,
    pub revision_status: Signal<Option<SharedString>>,
    pub revision_busy: Signal<bool>,
    owner: Owner,
    vault: CredentialVault,
    pending_review: Rc<RefCell<Option<PreparedDocumentReviewRequest>>>,
    review_origin: Rc<RefCell<Option<ReviewOrigin>>>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewSection {
    pub title: String,
    pub items: Vec<String>,
}

/// One content-search row.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub label: String,
}

impl AppState {
    pub fn new() -> Self {
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
        let status = notice
            .clone()
            .unwrap_or_else(|| SharedString::from("Open a folder to begin."));
        Self {
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
            settings: signal(SettingsData::load()),
            settings_open: signal(false),
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
            review_view: signal(None),
            review_busy: signal(false),
            translation_busy: signal(false),
            revision_questions: signal(Vec::new()),
            revision_consent: signal(None),
            revision_hunks: signal(Vec::new()),
            revision_status: signal(None),
            revision_busy: signal(false),
            vault: CredentialVault::production(),
            pending_review: Rc::new(RefCell::new(None)),
            review_origin: Rc::new(RefCell::new(None)),
            revision_context: Rc::new(RefCell::new(None)),
            pending_revision: Rc::new(RefCell::new(None)),
            revision_proposal: Rc::new(RefCell::new(None)),
            recovery,
            recovery_tracks: Rc::new(RefCell::new(HashMap::new())),
            quit_discarded: Rc::new(Cell::new(false)),
            next_id: Rc::new(Cell::new(1)),
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
                self.status
                    .set(SharedString::from(format!("Opened {}", path.display())));
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
        });
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
        let active_keys: HashSet<RecoveryKey> = tabs
            .iter()
            .map(|tab| RecoveryKey::for_path(&tab.path))
            .collect();
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
            let checkpoint = RecoveryCheckpoint {
                key: RecoveryKey::for_path(&tab.path),
                text: tab.text.get().to_string(),
                metadata: RecoveryMetadata::from_loaded_file(&tab.loaded.borrow()),
                revision: None,
            };
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
        let active_keys: HashSet<RecoveryKey> = tabs
            .iter()
            .map(|tab| RecoveryKey::for_path(&tab.path))
            .collect();
        for tab in tabs {
            let checkpoint = RecoveryCheckpoint {
                key: RecoveryKey::for_path(&tab.path),
                text: tab.text.get().to_string(),
                metadata: RecoveryMetadata::from_loaded_file(&tab.loaded.borrow()),
                revision: None,
            };
            let _ = store.checkpoint(&checkpoint, &active_keys);
        }
    }

    /// Delete the checkpoint for a path once its text is saved or the tab is
    /// closed deliberately.
    fn retire_recovery(&self, path: &Path) {
        if let Some(store) = &self.recovery {
            let _ = store.invalidate_and_delete(&RecoveryKey::for_path(path));
        }
    }

    /// Reopen a recovered buffer as a dirty tab. Its original stamp is kept so
    /// Save still refuses to overwrite a file changed since the checkpoint.
    pub fn restore_recovered(&self, record: &RecoveredRecord) {
        let key = record.record.key.clone();
        self.add_tab_with(recovered_loaded(record), true);
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
        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title("Unsaved changes")
                .description("Save this document before closing it?")
                .action("save", "Save")
                .destructive("discard", "Discard")
                .cancel("cancel", "Cancel")
                .show()
                .await;
            match answer {
                Ok(answer) if answer.action.as_deref() == Some("save") => {
                    state.set_active(id);
                    state.save_active();
                    if !state
                        .tabs
                        .get()
                        .iter()
                        .find(|tab| tab.id == id)
                        .is_some_and(|tab| tab.dirty.get())
                    {
                        state.close_tab_now(id);
                    }
                }
                Ok(answer) if answer.action.as_deref() == Some("discard") => {
                    state.close_tab_now(id);
                }
                Ok(_) => {}
                Err(error) => state
                    .status
                    .set(SharedString::from(format!("Close dialog failed: {error}"))),
            }
        });
    }

    fn close_tab_now(&self, id: u64) {
        if let Some(path) = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.path.clone())
        {
            self.retire_recovery(&path);
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
            state.retire_recovery(&tab.path);
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
                    state.retire_recovery(&tab.path);
                    state.recovery_tracks.borrow_mut().remove(&tab.id);
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
        let name = tab.title.to_string();
        let state = self.clone();
        let picked = pick_save_path(SaveDialog::new().title("Save As").file_name(name).filter(
            nsh_core::file_dialog::FileFilter::new("Markdown", ["md", "markdown"]),
        ));
        self.spawn_task(async move {
            match picked.await {
                Ok(Some(path)) => {
                    let text = tab.text.get().to_string();
                    let (newline, had_bom) = {
                        let loaded = tab.loaded.borrow();
                        (loaded.newline, loaded.had_bom)
                    };
                    match io::save_as(&path, &text, newline, had_bom) {
                        Ok(loaded) => {
                            state.close_tab_now(tab.id);
                            state.add_tab(loaded);
                        }
                        Err(SaveError::DestinationExists) => state.status.set(SharedString::from(
                            "Save As refused: that file exists. Choose a new name.",
                        )),
                        Err(error) => state
                            .status
                            .set(SharedString::from(format!("Save As failed: {error:?}"))),
                    }
                }
                Ok(None) => {}
                Err(error) => state.status.set(SharedString::from(format!(
                    "Save As picker failed: {error}"
                ))),
            }
        });
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
        self.save_tab(tab.id);
    }

    fn save_tab(&self, id: u64) -> bool {
        let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id).cloned() else {
            return false;
        };
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
                self.retire_recovery(&tab.path);
                self.recovery_tracks.borrow_mut().remove(&tab.id);
                self.status
                    .set(SharedString::from(format!("Saved {}", tab.path.display())));
                true
            }
            Err(SaveError::Conflict) => {
                tab.conflict.set(true);
                self.status.set(SharedString::from(
                    "Save refused: the file changed on disk. Reload, overwrite, or save as a new file.",
                ));
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
    /// is the guarded close path available inside the app; the platform title
    /// bar and Alt+F4 still have no cancellable close event in nsh 0.4, so the
    /// periodic checkpoint remains the only recovery protection on that path.
    /// A dirty buffer is checkpointed before the dialog as a best-effort extra
    /// guard against a failed save or dialog.
    pub fn request_quit(&self) {
        let dirty: Vec<u64> = self
            .tabs
            .get()
            .iter()
            .filter(|tab| tab.dirty.get())
            .map(|tab| tab.id)
            .collect();
        if dirty.is_empty() {
            nsh_core::element::desktop::close_window();
            return;
        }
        self.checkpoint_dirty_now();

        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title("Quit MarkTurbo?")
                .description("Some documents have unsaved changes.")
                .action("save", "Save all")
                .destructive("discard", "Discard all")
                .cancel("cancel", "Cancel")
                .show()
                .await;
            match answer {
                Ok(answer) if answer.action.as_deref() == Some("save") => {
                    for &id in &dirty {
                        if !state.save_tab(id) {
                            state.status.set(SharedString::from(
                                "Quit cancelled: a document could not be saved.",
                            ));
                            return;
                        }
                    }
                    nsh_core::element::desktop::close_window();
                }
                Ok(answer) if answer.action.as_deref() == Some("discard") => {
                    state.quit_discarded.set(true);
                    for id in &dirty {
                        if let Some(tab) = state.tabs.get().iter().find(|tab| tab.id == *id) {
                            state.retire_recovery(&tab.path);
                        }
                    }
                    nsh_core::element::desktop::close_window();
                }
                Ok(_) => {}
                Err(error) => state
                    .status
                    .set(SharedString::from(format!("Quit dialog failed: {error}"))),
            }
        });
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
                        state.status.set(SharedString::from(
                            crate::i18n::t(language, crate::i18n::Key::ModelRequestDocumentClosed)
                                .to_owned(),
                        ));
                        return;
                    };
                    if origin.snapshot.revision != tab.revision.get() {
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
                Ok(Err(error)) => state
                    .status
                    .set(SharedString::from(format!("Translation failed: {error}"))),
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
        let Some(tab) = self.active_tab() else {
            return;
        };
        let origin = ReviewOrigin {
            tab_id: tab.id,
            snapshot: SourceSnapshot::new(tab.revision.get(), 0),
        };
        self.context_open.set(false);
        self.review_view.set(None);
        self.review_consent.set(None);
        *self.review_origin.borrow_mut() = None;
        self.reset_revision();

        let settings = self.settings.get();
        let language = match settings.language {
            Language::Chinese => ReviewLanguage::SimplifiedChinese,
            Language::English => ReviewLanguage::English,
        };
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
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
        *self.pending_review.borrow_mut() = Some(bound);
        *self.review_origin.borrow_mut() = Some(origin);
        self.review_consent
            .set(Some(SharedString::from(disclosure)));
        self.review_open.set(true);
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
        self.review_busy.set(true);
        let state = self.clone();
        let revision = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == origin.tab_id)
            .map(|tab| tab.revision)
            .unwrap_or_else(|| signal(0));
        self.spawn_task(async move {
            let result = blocking(move || bound.execute(authorization)).await;
            state.review_busy.set(false);
            let view = match result {
                Ok(Ok(transport)) => {
                    let stale = transport.result.snapshot.revision != revision.get();
                    if let Some(output) = transport.result.output.clone() {
                        state.set_revision_context(
                            origin,
                            revision_request,
                            output,
                            revision_language,
                        );
                    }
                    ReviewView::from_result(&transport.result, stale)
                }
                Ok(Err(error)) => ReviewView::from_error(&error),
                Err(error) => ReviewView {
                    status: "Review failed".to_owned(),
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
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
    }

    fn reset_revision(&self) {
        *self.revision_context.borrow_mut() = None;
        *self.pending_revision.borrow_mut() = None;
        *self.revision_proposal.borrow_mut() = None;
        self.revision_questions.set(Vec::new());
        self.revision_consent.set(None);
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
        self.revision_busy.set(false);
    }

    /// Build a Revision request from the frozen Review output and the answer
    /// states shown in the panel, then disclose it before any send.
    pub fn start_revision(&self) {
        let context = self.revision_context.borrow().as_ref().map(|context| {
            (
                context.request.clone(),
                context.output.clone(),
                context.language,
            )
        });
        let Some((request, output, language)) = context else {
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
        *self.pending_revision.borrow_mut() = Some(bound);
    }

    pub fn cancel_revision(&self) {
        *self.pending_revision.borrow_mut() = None;
        self.revision_consent.set(None);
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
        self.revision_busy.set(true);
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
                Ok(Err(error)) => state.revision_status.set(Some(SharedString::from(format!(
                    "Revision failed: {error}"
                )))),
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
        if text == tab.text.get().as_str() {
            self.revision_status
                .set(Some(SharedString::from("No changes approved.")));
            return;
        }
        tab.text.set(SharedString::from(text));
        tab.dirty.set(true);
        tab.revision.update(|revision| *revision += 1);
        self.note_edit(tab.id);
        *self.revision_proposal.borrow_mut() = None;
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(Some(SharedString::from(
            "Approved changes applied to the buffer.",
        )));
    }

    /// Withdraw the prepared request before it is sent.
    pub fn cancel_review(&self) {
        self.pending_review.borrow_mut().take();
        self.review_origin.borrow_mut().take();
        self.review_consent.set(None);
        self.review_busy.set(false);
    }

    pub fn close_review(&self) {
        self.cancel_review();
        self.reset_revision();
        self.review_view.set(None);
        self.review_open.set(false);
    }

    pub fn open_settings(&self) {
        self.settings_open.set(true);
    }

    pub fn close_settings(&self) {
        self.settings_open.set(false);
        self.save_settings();
    }

    pub fn save_settings(&self) {
        self.settings.get().save();
        self.status.set(SharedString::from("Settings saved."));
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
            .map_err(|error| error.to_string())
    }

    /// Store a key in the platform's secure store. Refused where the platform
    /// provides none, rather than falling back to plaintext.
    pub fn store_persistent_credential(&self, value: &str) -> Result<(), String> {
        if !self.vault.secure_store_supported() {
            return Err("Secure storage is unavailable on this platform; use a session key or an environment variable.".to_owned());
        }
        let target = self.credential_target()?;
        self.vault
            .replace_persistent(&target, value.to_owned())
            .map_err(|error| error.to_string())
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

impl ReviewView {
    fn from_result(result: &ReviewResult, stale: bool) -> Self {
        let mut view = Self {
            status: if stale {
                "Stale".to_owned()
            } else {
                match result.status {
                    ReviewStatus::Ready => "Ready".to_owned(),
                    ReviewStatus::Stale => "Stale".to_owned(),
                    ReviewStatus::Diagnostic => "Diagnostic".to_owned(),
                }
            },
            diagnostic: result
                .diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.message.as_str().to_owned()),
            ..Self::default()
        };
        let Some(output) = &result.output else {
            return view;
        };
        view.sections = sections(&output.understood_intent);
        view.findings = output
            .findings
            .iter()
            .map(|finding| {
                let kind = match finding.kind {
                    FindingKind::Source | FindingKind::SourceStatement => "Source",
                    FindingKind::Inference => "Inference",
                };
                format!("{kind}: {}", finding.text.as_str())
            })
            .collect();
        view.questions = output
            .clarification_questions
            .iter()
            .map(|question| {
                let priority = match question.priority {
                    ClarificationPriority::Critical => "Critical",
                    ClarificationPriority::High => "High",
                    ClarificationPriority::Medium => "Medium",
                    ClarificationPriority::Low => "Low",
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
        view
    }

    fn from_error(error: &ReviewError) -> Self {
        Self {
            status: "Review failed".to_owned(),
            diagnostic: Some(error.to_string()),
            ..Self::default()
        }
    }
}

fn sections(intent: &mt_core::review::ReviewSections) -> Vec<ReviewSection> {
    let list = |title: &str, items: &[mt_core::review::StructuredText]| ReviewSection {
        title: title.to_owned(),
        items: items.iter().map(|item| item.as_str().to_owned()).collect(),
    };
    [
        Some(ReviewSection {
            title: "Goal".to_owned(),
            items: vec![intent.stated_goal.as_str().to_owned()],
        }),
        Some(list("Context", &intent.relevant_context)),
        Some(list("Constraints", &intent.constraints)),
        Some(list("Non-goals", &intent.non_goals)),
        Some(ReviewSection {
            title: "Deliverable".to_owned(),
            items: vec![intent.expected_deliverable.as_str().to_owned()],
        }),
        Some(list("Success evidence", &intent.success_evidence)),
        Some(list("Assumptions", &intent.inferred_assumptions)),
        Some(list("Open decisions", &intent.unresolved_decisions)),
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
