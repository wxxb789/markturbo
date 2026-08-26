//! Filesystem watching.
//!
//! Detects external changes so the app can offer a safe reload instead of
//! silently working from stale text. Debounced: an agent rewriting a tree
//! produces bursts of events, and re-reading per event would thrash.

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
        })
    }

    /// Drain pending changes without blocking.
    ///
    /// Returns an empty vec when nothing happened, so this is safe to poll from
    /// a UI tick.
    pub fn poll(&self) -> Vec<Change> {
        let mut changes = Vec::new();
        while let Ok(events) = self.rx.try_recv() {
            for event in events {
                changes.extend(classify(&event, &self.root));
            }
        }
        dedup(changes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Directory churn this app never reports.
///
/// Without it, a `cargo build` or an `npm install` inside the workspace floods
/// the UI with reload prompts. The list is [`mt_doc::walk::SKIP_DIRS`] — a
/// directory the file tree will not show and the search will not walk has no
/// business waking the app up either, and this list having been the shortest
/// of the five meant `.venv` and `__pycache__` churn reached the UI while the
/// tree that would have displayed it did not.
use mt_doc::walk::is_noise_path as is_noise;

fn classify(event: &DebouncedEvent, root: &Path) -> Vec<Change> {
    use notify::EventKind;

    event
        .paths
        .iter()
        .filter(|p| !is_noise(p, root))
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

    /// Wait for a change matching `pred`, or give up.
    ///
    /// Filesystem notifications are inherently asynchronous and platform
    /// dependent, so polling with a deadline is the only reliable shape.
    fn wait_for(watcher: &Watcher, pred: impl Fn(&Change) -> bool) -> Option<Change> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(found) = watcher.poll().into_iter().find(&pred) {
                return Some(found);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn detects_an_external_modification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one\n").unwrap();

        let watcher = Watcher::new(dir.path()).unwrap();
        // Let the watcher establish itself before mutating.
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(&path, "two\n").unwrap();

        let change = wait_for(&watcher, |c| c.path().ends_with("a.md"));
        assert!(change.is_some(), "expected a change for a.md");
    }

    #[test]
    fn ignores_noise_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        let watcher = Watcher::new(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.path().join("node_modules/pkg.md"), "x").unwrap();
        std::fs::write(dir.path().join("real.md"), "x").unwrap();

        let change = wait_for(&watcher, |c| c.path().ends_with("real.md"));
        assert!(change.is_some(), "real file must be reported");
        // And nothing from node_modules ever surfaced.
        assert!(
            watcher
                .poll()
                .iter()
                .all(|c| !c.path().to_string_lossy().contains("node_modules"))
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
