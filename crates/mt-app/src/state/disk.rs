//! Live disk changes: the watcher's lifecycle, the event drain, and what an
//! external change does to an open document or the workspace tree.
//!
//! One `notify` watcher serves the app: the workspace root is watched
//! recursively from the moment it opens, each open document outside the root
//! has its own directory watched non-recursively, and a symbolic link's
//! resolved target directory is watched too. The channel the debouncer fills
//! is drained by [`AppState::disk_tick`], which `app.rs` calls from its 1 s
//! ticker (beside the recovery checkpoints) — draining is not a file-system
//! poll: no stat happens until an event says a path changed.

use super::*;
use mt_core::workspace::{Change, Watcher};

impl AppState {
    /// Why live watching is off, when it could not start. `None` once the
    /// watcher runs.
    pub fn watcher_diagnostic(&self) -> Option<String> {
        self.watcher_issue.borrow().clone()
    }

    /// Create the watcher on first need. A watcher that cannot start is
    /// recorded as a diagnostic and never blocks the user: saving keeps its
    /// own stamp check, so a stale buffer still cannot clobber a file.
    fn ensure_watcher(&self) -> bool {
        if self.watcher.borrow().is_some() {
            return true;
        }
        match Watcher::new() {
            Ok(watcher) => {
                *self.watcher.borrow_mut() = Some(watcher);
                *self.watcher_issue.borrow_mut() = None;
                true
            }
            Err(error) => {
                self.record_watcher_issue(format!("File watcher unavailable: {error}"));
                false
            }
        }
    }

    fn record_watcher_issue(&self, issue: String) {
        log::warn!("{issue}; external changes are checked on save only");
        *self.watcher_issue.borrow_mut() = Some(issue);
    }

    fn watcher_error(&self, action: &str, error: &dyn std::fmt::Display) {
        self.record_watcher_issue(format!("File watcher could not {action}: {error}"));
    }

    /// Watch the workspace root recursively, and re-derive which document
    /// directories an extra watch needs.
    pub(crate) fn watch_workspace_root(&self, root: &Path) {
        if !self.ensure_watcher() {
            return;
        }
        let result = self
            .watcher
            .borrow_mut()
            .as_mut()
            .map(|watcher| watcher.set_root(root));
        match result {
            Some(Ok(())) => {}
            Some(Err(error)) => self.watcher_error("watch the workspace", &error),
            None => {}
        }
        self.sync_document_watches();
    }

    /// Match the non-recursive watches to the open tabs' directories: each
    /// file tab's own parent, and a symbolic link's resolved target's parent,
    /// so a target outside the workspace is watched even when the link is
    /// inside it. The set is bounded by the open tabs, and a directory loses
    /// its watch when its last tab closes.
    pub(crate) fn sync_document_watches(&self) {
        if !self.ensure_watcher() {
            return;
        }
        let mut directories: HashSet<PathBuf> = HashSet::new();
        for tab in self.tabs.get_untracked() {
            if tab.untitled {
                continue;
            }
            if let Some(parent) = tab.path.parent() {
                directories.insert(parent.to_path_buf());
            }
            // A symbolic link's target is watched where it really lives, in
            // addition to the link's own location (decision 4). A regular
            // file's own parent is the whole story.
            let loaded = tab.loaded.borrow();
            if let SourceIdentity::SymbolicLink {
                resolved_target, ..
            } = &loaded.source_identity
                && let Some(parent) = resolved_target.parent()
            {
                directories.insert(parent.to_path_buf());
            }
        }
        let result = self
            .watcher
            .borrow_mut()
            .as_mut()
            .map(|watcher| watcher.sync_document_directories(directories));
        match result {
            Some(Ok(())) => {}
            Some(Err(error)) => self.watcher_error("watch a document directory", &error),
            None => {}
        }
    }

    /// The non-recursive document directories watched right now, for a probe
    /// that pins the watch scope.
    #[cfg(test)]
    pub(crate) fn watched_document_directories(&self) -> Vec<PathBuf> {
        self.watcher
            .borrow()
            .as_ref()
            .map(|watcher| watcher.watched_document_directories())
            .unwrap_or_default()
    }

    /// Whether `tab` reads `path`, directly or through a symbolic link whose
    /// recorded target resolves there.
    fn tab_reads_path(tab: &Tab, path: &Path) -> bool {
        if tab.untitled {
            return false;
        }
        if tab.path == path || io::paths_match(&tab.path, path) {
            return true;
        }
        let resolved = {
            let loaded = tab.loaded.borrow();
            match &loaded.source_identity {
                SourceIdentity::SymbolicLink {
                    resolved_target, ..
                } => resolved_target.clone(),
                SourceIdentity::Regular => return false,
            }
        };
        resolved == path || io::paths_match(&resolved, path)
    }

    /// Drain the watcher's debounced batches. Called from the app's disk
    /// ticker; an empty poll is a channel read, nothing more.
    pub fn disk_tick(&self) {
        let changes = {
            let watcher = self.watcher.borrow();
            match watcher.as_ref() {
                Some(watcher) => watcher.poll(),
                None => return,
            }
        };
        if changes.is_empty() {
            return;
        }
        let mut refresh_tree = false;
        for change in &changes {
            refresh_tree |= self.apply_disk_change(change);
        }
        if refresh_tree {
            self.refresh_tree_from_disk();
        }
    }

    /// Apply one change; true when the workspace tree's shape changed.
    fn apply_disk_change(&self, change: &Change) -> bool {
        let path = change.path();
        let in_root = self
            .root
            .get_untracked()
            .is_some_and(|root| path.starts_with(&root));
        let affects_tree = in_root && change.affects_tree();
        for tab in self.tabs.get_untracked() {
            if Self::tab_reads_path(&tab, path) {
                self.handle_document_change(&tab);
            }
        }
        affects_tree
    }

    fn handle_document_change(&self, tab: &Tab) {
        let generation = tab.disk_generation.get().wrapping_add(1);
        tab.disk_generation.set(generation);
        if !std::fs::metadata(&tab.path).is_ok_and(|metadata| metadata.is_file()) {
            // Moved or deleted: the banner offers Save As and keeps the
            // buffer. The generation bump above drops any read still in
            // flight, so an older read cannot clear this notice.
            tab.disk_missing.set(true);
            return;
        }
        tab.disk_missing.set(false);
        if tab.dirty.get() {
            // Unsaved edits: raise the conflict at once, not at save.
            self.note_conflict(tab);
            return;
        }
        self.start_disk_reload(tab);
    }

    /// Read `tab`'s file and adopt it if nothing about the tab moved while
    /// the read was in flight.
    fn start_disk_reload(&self, tab: &Tab) {
        let generation = tab.disk_generation.get();
        let revision = tab.revision.get();
        let state = self.clone();
        let tab = tab.clone();
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
            // The answer belongs to the tab it was read for, unchanged since
            // the read began: a newer disk event — a delete or a move —
            // refuses it, and so does an edit made while it was read.
            if tab.disk_generation.get() != generation {
                return;
            }
            let Some(current) = state
                .tabs
                .get()
                .iter()
                .find(|current| current.id == tab.id)
                .cloned()
            else {
                return;
            };
            let loaded = match answer {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        current.disk_missing.set(true);
                    } else {
                        state.notify_failure(crate::i18n::Key::ReloadFailed, &error);
                    }
                    return;
                }
                Err(error) => {
                    state.notify_failure(crate::i18n::Key::ReloadFailed, &error);
                    return;
                }
            };
            if current.revision.get() != revision {
                state.note_conflict(&current);
                return;
            }
            let unchanged = {
                let loaded_before = current.loaded.borrow();
                loaded_before.stamp == loaded.stamp
            };
            if unchanged {
                // The stamp matches our own last write: the app's save is
                // never an external change, so there is nothing to reload and
                // nothing to say.
                return;
            }
            state.adopt_disk_version(&current, loaded);
            state.notify(SharedString::from(s::updated_from_disk(
                state.settings.get().language,
            )));
        });
    }

    /// Re-scan the workspace tree after a create, delete or rename, off the
    /// UI thread. The scan commits only if it is still the latest refresh for
    /// the same root.
    fn refresh_tree_from_disk(&self) {
        let Some(root) = self.root.get_untracked() else {
            return;
        };
        let request = self.tree_refresh.get().wrapping_add(1);
        self.tree_refresh.set(request);
        let state = self.clone();
        self.spawn_task(async move {
            let scan_root = root.clone();
            let scanned = blocking(move || tree::read_dir_deep(&scan_root, 4)).await;
            if state.tree_refresh.get() != request
                || state.root.get_untracked().as_deref() != Some(root.as_path())
            {
                return;
            }
            let Ok(Ok(nodes)) = scanned else {
                return;
            };
            let mut entries = Vec::new();
            flatten(&nodes, 0, &mut entries);
            state.entries.set(entries);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    /// Renders what the watcher recorded, so a headless test can see the
    /// diagnostic the app deliberately keeps off its UI.
    #[component]
    fn WatcherIssueProbe() -> impl IntoElement {
        let state = AppState::new();
        // A root that cannot be watched: the watcher is created (no root),
        // then the root watch fails and the diagnostic is recorded.
        let missing = std::env::temp_dir().join("markturbo-22b-no-such-folder");
        assert!(!missing.exists());
        state.watch_workspace_root(&missing);
        let shown = state.clone();
        view().test_id("watcher-issue").child(text(move || {
            SharedString::from(shown.watcher_diagnostic().unwrap_or_default())
        }))
    }

    #[test]
    fn a_watcher_that_cannot_start_records_a_diagnostic_and_blocks_nothing() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<WatcherIssueProbe>()))
            .expect("start the probe");
        app.wait_for("text:/File watcher could not/")
            .expect("a failed root watch is recorded as a diagnostic");
        // The app stays usable: a drain with no watch is a no-op.
        app.wait(std::time::Duration::from_millis(600))
            .expect("the app keeps running");
    }

    /// A probe that opens one document outside any workspace, renders how many
    /// document directories are watched, and closes that tab on a press.
    #[component]
    fn DocumentWatchProbe() -> impl IntoElement {
        let state = AppState::new();
        let directory = std::rc::Rc::new(tempfile::tempdir().unwrap());
        let path = directory.path().join("watched.md");
        std::fs::write(&path, "# watched\n").unwrap();
        let id = state.add_tab(mt_core::document::io::load(&path).unwrap());
        // The watch list is plain state, not a signal: the probe reads it
        // once now and again when the tab closes, and renders each reading.
        let watched = signal(SharedString::from(format!(
            "watched:{}",
            state.watched_document_directories().len()
        )));
        let close_state = state.clone();
        let close_watched = watched.clone();
        let keep = directory.clone();
        view()
            .test_id("watch-scope")
            .child(text(move || watched.get()))
            .child(
                view()
                    .test_id("close-watched")
                    .on_press(move |_: &PressEvent| {
                        let _ = &keep;
                        close_state.close_tab_now(id);
                        close_watched.set(SharedString::from(format!(
                            "watched:{}",
                            close_state.watched_document_directories().len()
                        )));
                    })
                    .child(text(|| "Close")),
            )
    }

    #[test]
    fn a_tabs_document_directory_is_watched_until_the_last_tab_closes() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<DocumentWatchProbe>()))
            .expect("start the probe");
        app.wait_for("text:watched:1")
            .expect("the tab's directory is watched");
        app.press("id:close-watched").expect("close the tab");
        app.wait_for("text:watched:0")
            .expect("the last tab's directory loses its watch");
    }
}
