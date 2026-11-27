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
            // flight, so an older read cannot clear this notice, and an open
            // comparison loses the version that no longer exists.
            tab.disk_missing.set(true);
            self.invalidate_comparison(tab);
            return;
        }
        tab.disk_missing.set(false);
        self.start_disk_reload(tab);
    }

    /// Read `tab`'s file and adopt it if nothing about the tab moved while
    /// the read was in flight. The read decides what kind of change this is:
    /// the stamp compared against the tab's last read or write is what tells
    /// the app's own save from an external one, before any dirty flag does.
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
            // Our own last read or write: the app's save is never an external
            // change, even when the user typed again before the save's event
            // was drained, so this check comes before the revision one.
            let own_write = {
                let loaded_before = current.loaded.borrow();
                loaded_before.stamp == loaded.stamp
            };
            if own_write {
                return;
            }
            if current.revision.get() != revision {
                state.note_conflict_of(&current, Some(&loaded.stamp));
                return;
            }
            if current.dirty.get() {
                state.note_conflict_of(&current, Some(&loaded.stamp));
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
    use nsh_base::editor::{EditorPosition, editor};
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;
    use crate::ui::tokens;

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

    const ANCHOR_OLD: &str = "line zero\nline one\nline two\nline three\nline four\ntail\n";

    /// Line 4 stays, but the four lines above it grow far past the caret's
    /// old byte offset: offset mapping would leave the caret on line 0.
    fn anchor_new() -> String {
        let long = "A".repeat(120);
        format!("{long}\n{long}\n{long}\n{long}\nline four\ntail\n")
    }

    /// Mounts one document's editor, sets a caret, adopts a new disk version,
    /// and renders the caret read back after the anchor restore's frames.
    #[component]
    fn ReloadAnchorProbe() -> impl IntoElement {
        let state = AppState::new();
        let directory = std::rc::Rc::new(tempfile::tempdir().unwrap());
        let path = directory.path().join("anchor.md");
        std::fs::write(&path, ANCHOR_OLD).unwrap();
        let id = state.add_tab(io::load(&path).unwrap());
        let editor_ref = component_ref::<Editor>();
        state.attach_source_editor(id, editor_ref);
        let tab = state
            .tabs
            .get_untracked()
            .into_iter()
            .find(|tab| tab.id == id)
            .expect("the tab");
        let readback = signal(SharedString::from("caret:?"));
        let press_state = state.clone();
        let press_readback = readback;
        let press_tab = tab.clone();
        let keep = directory.clone();
        view()
            .test_id("anchor-probe")
            .child(
                editor()
                    .component_ref(&editor_ref)
                    .value({
                        let tab = tab.clone();
                        move || Some(tab.text.get())
                    })
                    .font_family(tokens::FONT_SOURCE)
                    .font_size(px(tokens::TYPE_SOURCE))
                    .line_height(px(tokens::TYPE_SOURCE_LINE))
                    .w(px(600.0))
                    .h(px(400.0)),
            )
            .child(
                view()
                    .test_id("anchor-reload")
                    .on_press(move |_: &PressEvent| {
                        let _ = &keep;
                        if let Some(api) = editor_ref.api::<EditorApi>() {
                            api.set_selections(vec![EditorSelection::caret(EditorPosition::new(
                                4, 1,
                            ))]);
                        }
                        std::fs::write(&path, anchor_new()).unwrap();
                        let loaded = io::load(&path).unwrap();
                        press_state.adopt_disk_version(&press_tab, loaded);
                        let readback = press_readback;
                        press_state.spawn_task(async move {
                            for _ in 0..6 {
                                request_animation_frame(|_| {}).await;
                            }
                            let shown = editor_ref
                                .api::<EditorApi>()
                                .and_then(|api| api.selections().pop())
                                .map(|selection| {
                                    format!(
                                        "caret:{}:{}",
                                        selection.head.line, selection.head.column
                                    )
                                })
                                .unwrap_or_else(|| "caret:none".to_owned());
                            readback.set(SharedString::from(shown));
                        });
                    })
                    .child(text(|| "Reload")),
            )
            .child(text(move || readback.get()))
    }

    #[test]
    fn a_reload_keeps_the_caret_line_by_row_and_column() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ReloadAnchorProbe>()))
            .expect("start the probe");
        app.wait_for("id:anchor-reload").expect("the probe");
        app.press("id:anchor-reload").expect("reload");
        app.wait_for("text:caret:4:1")
            .expect("the caret stays on its line after the reload");
    }
}
