//! Filesystem watching.
//!
//! Detects external changes so the app can offer a safe reload instead of
//! silently working from stale text. Debounced: an agent rewriting a tree
//! produces bursts of events, and re-reading per event would thrash.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebouncedEvent, Debouncer, NoCache, new_debouncer_opt};

/// What changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file's content may have changed; the caller should re-stamp it.
    Modified(PathBuf),
    /// A path appeared.
    Created(PathBuf),
    /// A path disappeared.
    Removed(PathBuf),
}

impl Change {
    pub fn path(&self) -> &Path {
        match self {
            Change::Modified(p) | Change::Created(p) | Change::Removed(p) => p,
        }
    }

    /// True when the file tree's shape changed, so the explorer needs a refresh.
    pub fn affects_tree(&self) -> bool {
        !matches!(self, Change::Modified(_))
    }
}

/// Watches a workspace directory.
///
/// The watcher must be kept alive; dropping it stops the notifications.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher, NoCache>,
    rx: Receiver<Vec<DebouncedEvent>>,
    root: PathBuf,
    document_directories: HashSet<PathBuf>,
    context_paths: HashSet<PathBuf>,
}

/// Debounce window. Long enough to coalesce an agent's multi-file write, short
/// enough that a human save feels immediate.
const DEBOUNCE: Duration = Duration::from_millis(300);

impl Watcher {
    /// Start watching `root` recursively.
    ///
    /// Uses [`NoCache`] rather than the platform default. On Windows the
    /// default is `FileIdMap`, whose `add_path` walks the entire tree opening a
    /// handle per file to record its file id — measured at **22 seconds** on a
    /// 3,182-file vault, synchronously, before the window can draw. It also
    /// walks `.git` and `node_modules`, which this module already treats as
    /// noise.
    ///
    /// The cache buys one thing: correlating a rename's two halves by file id
    /// when the platform supplies no rename tracker. Windows and macOS both do
    /// supply one, and `classify` derives the same answer from whether the
    /// path still exists — so the walk paid for a fallback that is never
    /// reached.
    pub fn new(root: &Path) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            DEBOUNCE,
            None,
            move |result| {
                // A closed receiver means the app is shutting down; drop
                // silently.
                if let Ok(events) = result {
                    let _ = tx.send(events);
                }
            },
            NoCache,
            notify::Config::default(),
        )?;
        debouncer.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _debouncer: debouncer,
            rx,
            root: root.to_path_buf(),
            document_directories: HashSet::new(),
            context_paths: HashSet::new(),
        })
    }

    /// Match the non-recursive watches to the parents of open documents.
    ///
    /// Save As and Open File can give a document a path outside the workspace
    /// tree. Those parents remain bounded by the open tab set and disappear
    /// when the last document using them closes or moves.
    pub fn sync_document_directories(
        &mut self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> anyhow::Result<()> {
        let desired: HashSet<_> = directories
            .into_iter()
            .filter(|directory| !directory.starts_with(&self.root))
            .collect();

        let removed: Vec<_> = self
            .document_directories
            .difference(&desired)
            .cloned()
            .collect();
        for directory in removed {
            self._debouncer.unwatch(&directory)?;
            self.document_directories.remove(&directory);
        }

        let added: Vec<_> = desired
            .difference(&self.document_directories)
            .cloned()
            .collect();
        for directory in added {
            self._debouncer
                .watch(&directory, RecursiveMode::NonRecursive)?;
            self.document_directories.insert(directory);
        }
        Ok(())
    }

    /// Replace the exact context candidate and root-marker paths to report,
    /// including missing paths and paths normally filtered as noise.
    ///
    /// This does not add watches or include descendants. The workspace must
    /// combine these paths' parents with open-document directories when calling
    /// [`Self::sync_document_directories`]. Paths must use the same spelling as
    /// the watched paths so they match filesystem events.
    pub fn sync_context_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.context_paths = paths.into_iter().collect();
    }

    /// Drain pending changes without blocking.
    ///
    /// Returns an empty vec when nothing happened, so this is safe to poll from
    /// a UI tick.
    pub fn poll(&self) -> Vec<Change> {
        let mut changes = Vec::new();
        while let Ok(events) = self.rx.try_recv() {
            for event in events {
                changes.extend(classify(
                    &event,
                    &self.root,
                    &self.document_directories,
                    &self.context_paths,
                ));
            }
        }
        dedup(changes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Directory churn a workspace watcher filters unless an exact context path
/// has been registered.
///
/// Without it, a `cargo build` or an `npm install` inside the workspace floods
/// the UI with reload prompts. The list is [`super::walk::SKIP_DIRS`] — a
/// directory the file tree will not show and the search will not walk has no
/// business waking the app up either, and this list having been the shortest
/// of the five meant `.venv` and `__pycache__` churn reached the UI while the
/// tree that would have displayed it did not.
use super::walk::is_noise_path as is_noise;

fn classify(
    event: &DebouncedEvent,
    root: &Path,
    document_directories: &HashSet<PathBuf>,
    context_paths: &HashSet<PathBuf>,
) -> Vec<Change> {
    use notify::EventKind;

    event
        .paths
        .iter()
        .filter(|path| {
            if context_paths.contains(path.as_path()) {
                return true;
            }
            let root = std::iter::once(root)
                .chain(document_directories.iter().map(PathBuf::as_path))
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
                .unwrap_or(root);
            !is_noise(path, root)
        })
        .filter_map(|path| {
            let path = path.clone();
            Some(match event.kind {
                EventKind::Create(_) => Change::Created(path),
                EventKind::Remove(_) => Change::Removed(path),
                EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                    // A rename shows up as two paths; whichever still exists is
                    // the new name, the other is gone.
                    if path.exists() {
                        Change::Created(path)
                    } else {
                        Change::Removed(path)
                    }
                }
                EventKind::Modify(_) => Change::Modified(path),
                // Access events say nothing about content.
                _ => return None,
            })
        })
        .collect()
}

/// Collapse repeats, keeping the last verdict per path.
///
/// A save often arrives as create+modify; reporting both would prompt twice.
fn dedup(changes: Vec<Change>) -> Vec<Change> {
    let mut seen: Vec<Change> = Vec::with_capacity(changes.len());
    for change in changes {
        match seen.iter_mut().find(|c| c.path() == change.path()) {
            Some(existing) => *existing = change,
            None => seen.push(change),
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: notify::EventKind, paths: &[PathBuf]) -> DebouncedEvent {
        let event = paths.iter().fold(notify::Event::new(kind), |event, path| {
            event.add_path(path.clone())
        });
        DebouncedEvent::new(event, std::time::Instant::now())
    }

    #[test]
    fn context_marker_creation_and_removal_bypass_noise_filtering() {
        use notify::EventKind;
        use notify::event::{CreateKind, RemoveKind};

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(".git");
        let mut watcher = Watcher::new(dir.path()).unwrap();
        watcher.sync_context_paths([marker.clone()]);
        let (tx, rx) = channel();
        watcher.rx = rx;

        for kind in [
            EventKind::Create(CreateKind::Folder),
            EventKind::Create(CreateKind::File),
        ] {
            tx.send(vec![event(kind, std::slice::from_ref(&marker))])
                .unwrap();
            assert_eq!(watcher.poll(), vec![Change::Created(marker.clone())]);
        }
        for kind in [
            EventKind::Remove(RemoveKind::Folder),
            EventKind::Remove(RemoveKind::File),
        ] {
            tx.send(vec![event(kind, std::slice::from_ref(&marker))])
                .unwrap();
            assert_eq!(watcher.poll(), vec![Change::Removed(marker.clone())]);
        }
    }

    #[test]
    fn context_marker_rename_reports_both_registered_paths() {
        use notify::EventKind;
        use notify::event::{ModifyKind, RenameMode};

        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join(".git");
        let to = dir.path().join("nested/.git");
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::create_dir(&from).unwrap();
        let context_paths = HashSet::from([from.clone(), to.clone()]);
        let rename = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[from.clone(), to.clone()],
        );

        std::fs::rename(&from, &to).unwrap();
        assert_eq!(
            classify(&rename, dir.path(), &HashSet::new(), &context_paths),
            vec![Change::Removed(from.clone()), Change::Created(to.clone())]
        );
        std::fs::rename(&to, &from).unwrap();
        assert_eq!(
            classify(&rename, dir.path(), &HashSet::new(), &context_paths),
            vec![Change::Created(from), Change::Removed(to)]
        );
    }

    #[test]
    fn context_paths_do_not_allow_descendants_or_unregistered_noise() {
        use notify::EventKind;
        use notify::event::ModifyKind;

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(".git");
        let candidate = dir.path().join("node_modules/AGENTS.md");
        let real = dir.path().join("real.md");
        let changed = event(
            EventKind::Modify(ModifyKind::Any),
            &[
                marker.clone(),
                marker.join("HEAD"),
                dir.path().join("nested/.git"),
                candidate.clone(),
                dir.path().join("node_modules/pkg.md"),
                dir.path().join(".venv/AGENTS.md"),
                real.clone(),
            ],
        );
        let mut watcher = Watcher::new(dir.path()).unwrap();
        let (tx, rx) = channel();
        watcher.rx = rx;
        watcher.sync_context_paths([marker.clone(), candidate.clone()]);
        tx.send(vec![changed.clone()]).unwrap();
        assert_eq!(
            watcher.poll(),
            vec![
                Change::Modified(marker),
                Change::Modified(candidate.clone()),
                Change::Modified(real.clone()),
            ]
        );

        watcher.sync_context_paths([candidate.clone()]);
        tx.send(vec![changed.clone()]).unwrap();
        assert_eq!(
            watcher.poll(),
            vec![Change::Modified(candidate), Change::Modified(real.clone())]
        );
        watcher.sync_context_paths([]);
        tx.send(vec![changed]).unwrap();
        assert_eq!(watcher.poll(), vec![Change::Modified(real)]);
    }

    /// Receive the exact change from the channel registered before the write.
    fn receive_change(watcher: &Watcher, expected: &Change) -> Change {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let events = watcher
                .rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("expected filesystem notification before the deadline");
            let changes = dedup(
                events
                    .iter()
                    .flat_map(|event| {
                        classify(
                            event,
                            &watcher.root,
                            &watcher.document_directories,
                            &watcher.context_paths,
                        )
                    })
                    .collect(),
            );
            if let Some(found) = changes.into_iter().find(|change| change == expected) {
                return found;
            }
        }
    }

    #[test]
    fn detects_an_external_modification() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("a.md");
        std::fs::write(&path, "one\n").unwrap();

        let watcher = Watcher::new(&root).unwrap();
        std::fs::write(&path, "two\n").unwrap();

        let expected = Change::Modified(path);
        assert_eq!(receive_change(&watcher, &expected), expected);
    }

    #[test]
    fn detects_a_change_in_an_added_directory_outside_the_primary_root() {
        let primary = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let primary_root = primary.path().canonicalize().unwrap();
        let external_root = external.path().canonicalize().unwrap();
        let path = external_root.join("saved-as.md");
        std::fs::write(&path, "one\n").unwrap();

        let mut watcher = Watcher::new(&primary_root).unwrap();
        watcher.sync_document_directories([external_root]).unwrap();
        std::fs::write(&path, "two\n").unwrap();

        let expected = Change::Modified(path);
        assert_eq!(receive_change(&watcher, &expected), expected);
    }

    #[test]
    fn syncing_document_directories_drops_roots_no_open_document_uses() {
        let primary = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut watcher = Watcher::new(primary.path()).unwrap();

        watcher
            .sync_document_directories([first.path().to_path_buf(), second.path().to_path_buf()])
            .unwrap();
        assert_eq!(watcher.document_directories.len(), 2);

        watcher
            .sync_document_directories([second.path().to_path_buf()])
            .unwrap();
        assert_eq!(watcher.document_directories.len(), 1);
        assert!(watcher.document_directories.contains(second.path()));
    }

    #[test]
    fn ignores_noise_directories() {
        let root = Path::new("/w");
        let real = root.join("real.md");
        let created = event(
            notify::EventKind::Create(notify::event::CreateKind::File),
            &[root.join("node_modules/pkg.md"), real.clone()],
        );
        assert_eq!(
            classify(&created, root, &HashSet::new(), &HashSet::new()),
            vec![Change::Created(real)]
        );
    }

    #[test]
    fn poll_is_non_blocking_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = Watcher::new(dir.path()).unwrap();
        let start = std::time::Instant::now();
        assert!(watcher.poll().is_empty());
        assert!(start.elapsed() < Duration::from_millis(100), "poll blocked");
    }

    #[test]
    fn starting_a_watch_does_not_walk_the_tree() {
        // The defect this guards: on Windows the debouncer's default
        // `FileIdMap` cache walks every file under the root, opening a handle
        // each, before `new` returns — 22 seconds on a 3,182-file vault, on the
        // UI thread, before the window could draw. `NoCache` skips the walk.
        //
        // 200 files is enough that a per-file walk is unmistakable while
        // keeping the test itself quick.
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        for i in 0..200 {
            std::fs::write(deep.join(format!("f{i}.md")), "x").unwrap();
        }

        let start = std::time::Instant::now();
        let watcher = Watcher::new(dir.path()).unwrap();
        let elapsed = start.elapsed();
        drop(watcher);

        assert!(
            elapsed < Duration::from_millis(500),
            "registering the watch took {elapsed:?}; it must not scan the tree"
        );
    }

    #[test]
    fn dedup_keeps_the_last_verdict_per_path() {
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        let out = dedup(vec![
            Change::Created(a.clone()),
            Change::Modified(a.clone()),
            Change::Created(b.clone()),
        ]);
        assert_eq!(out, vec![Change::Modified(a), Change::Created(b)]);
    }

    #[test]
    fn tree_shape_changes_are_flagged() {
        let p = PathBuf::from("x");
        assert!(!Change::Modified(p.clone()).affects_tree());
        assert!(Change::Created(p.clone()).affects_tree());
        assert!(Change::Removed(p).affects_tree());
    }

    #[test]
    fn noise_detection_matches_nested_paths() {
        let root = Path::new("/w");
        assert!(is_noise(Path::new("/w/a/node_modules/b/c.md"), root));
        assert!(is_noise(Path::new("/w/.git/HEAD"), root));
        assert!(!is_noise(Path::new("/w/docs/target-audience.md"), root));
        // The defect consolidation fixes here: this list used to be the
        // shortest of five, so a `pip install` into `.venv` or a Python run
        // filling `__pycache__` produced reload prompts for files the tree
        // does not even show.
        assert!(is_noise(Path::new("/w/.venv/lib/x.py"), root));
        assert!(is_noise(Path::new("/w/src/__pycache__/m.pyc"), root));
    }
}
