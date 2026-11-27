//! Filesystem watching.
//!
//! Detects external changes so the app can offer a safe reload instead of
//! silently working from stale text. Debounced: an agent rewriting a tree
//! produces bursts of events, and re-reading per event would thrash.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    mpsc::{Receiver, Sender, channel},
};
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
/// Watches a workspace directory.
///
/// The watcher must be kept alive; dropping it stops the notifications.
///
/// Registration and removal run on one worker thread: `notify` queries the
/// directory and waits for its platform registration to complete, and a
/// volume that stops responding must block only that thread. The public
/// methods enqueue a command and return; the worker publishes what it
/// applied, and the UI classifies events by that applied state. A failed
/// registration is reported through [`Self::drain_problems`] instead of a
/// return value, so a caller on the UI thread never waits for the file
/// system.
pub struct Watcher {
    commands: Sender<Command>,
    rx: Receiver<Vec<DebouncedEvent>>,
    problems: Receiver<String>,
    /// What the worker applied: `poll` classifies events by this, never by a
    /// watch that is not live yet.
    applied: Arc<Mutex<AppliedWatches>>,
    context_paths: HashSet<PathBuf>,
}

enum Command {
    SetRoot(PathBuf),
    SyncDocumentDirectories(HashSet<PathBuf>),
    #[cfg(any(test, feature = "test-support"))]
    Flush(Sender<()>),
}

#[derive(Default)]
struct AppliedWatches {
    root: Option<PathBuf>,
    document_directories: HashSet<PathBuf>,
    root_generation: u64,
    directory_generation: u64,
}

struct WatchWorker {
    debouncer: Debouncer<RecommendedWatcher, NoCache>,
    root: Option<PathBuf>,
    canonical_root: Option<PathBuf>,
    desired_directories: HashSet<PathBuf>,
    document_directories: HashSet<PathBuf>,
    applied: Arc<Mutex<AppliedWatches>>,
    problems: Sender<String>,
}

/// Debounce window. Long enough to coalesce an agent's multi-file write, short
/// enough that a human save feels immediate.
const DEBOUNCE: Duration = Duration::from_millis(300);

impl WatchWorker {
    fn run(mut self, commands: Receiver<Command>) {
        while let Ok(command) = commands.recv() {
            match command {
                Command::SetRoot(root) => self.set_root(root),
                Command::SyncDocumentDirectories(directories) => {
                    self.desired_directories = directories;
                    self.refresh_document_directories();
                }
                #[cfg(any(test, feature = "test-support"))]
                Command::Flush(done) => {
                    let _ = done.send(());
                }
            }
        }
    }

    fn set_root(&mut self, root: PathBuf) {
        if self.root.as_deref() == Some(root.as_path()) {
            return;
        }
        if let Some(previous) = self.root.take()
            && let Err(error) = self.unwatch(&previous)
        {
            self.problem(format!(
                "File watcher could not stop watching the workspace: {error}"
            ));
        }
        if let Err(error) = self.watch(&root, RecursiveMode::Recursive) {
            self.problem(format!(
                "File watcher could not watch the workspace: {error}"
            ));
            // A root that could not be watched is never kept; the desired
            // directories fall back to their own non-recursive watches.
            self.publish_root(None);
            self.refresh_document_directories();
            return;
        }
        self.canonical_root = std::fs::canonicalize(&root)
            .ok()
            .map(|canonical| normalize_path(&canonical));
        self.root = Some(root.clone());
        self.refresh_document_directories();
        self.publish_root(Some(root));
    }

    fn refresh_document_directories(&mut self) {
        let wanted: HashSet<PathBuf> = self
            .desired_directories
            .iter()
            .filter(|directory| !self.covered(directory))
            .cloned()
            .collect();

        let removed: Vec<PathBuf> = self
            .document_directories
            .difference(&wanted)
            .cloned()
            .collect();
        for directory in removed {
            if let Err(error) = self.unwatch(&directory) {
                self.problem(format!(
                    "File watcher could not stop watching a document directory: {error}"
                ));
            }
            self.document_directories.remove(&directory);
        }

        let added: Vec<PathBuf> = wanted
            .difference(&self.document_directories)
            .cloned()
            .collect();
        for directory in added {
            if let Err(error) = self.watch(&directory, RecursiveMode::NonRecursive) {
                self.problem(format!(
                    "File watcher could not watch a document directory: {error}"
                ));
                continue;
            }
            self.document_directories.insert(directory);
        }

        let mut applied = self
            .applied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if applied.document_directories != self.document_directories {
            applied.document_directories = self.document_directories.clone();
            applied.directory_generation = applied.directory_generation.wrapping_add(1);
        }
    }

    fn covered(&self, directory: &Path) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| covered_by(directory, root))
            || self
                .canonical_root
                .as_deref()
                .is_some_and(|root| covered_by(directory, root))
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_probe::wait_for_watch_registration(path);
        self.debouncer.watch(path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_probe::wait_for_watch_registration(path);
        self.debouncer.unwatch(path)
    }

    fn publish_root(&self, root: Option<PathBuf>) {
        let mut applied = self
            .applied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        applied.root = root;
        applied.root_generation = applied.root_generation.wrapping_add(1);
    }

    fn problem(&self, problem: String) {
        let _ = self.problems.send(problem);
    }
}

/// The one spelling watches are registered and events are matched by.
///
/// `canonicalize` on Windows yields the verbatim form (`\\?\C:\notes`,
/// `\\?\UNC\server\share`), while the workspace and the event paths usually
/// use the plain form. A literal comparison of the two fails exactly where it
/// matters most — a removed path has no canonical spelling left — so the
/// verbatim prefix is stripped before anything is registered or matched. The
/// spelling's case is kept: a directory with the case-sensitivity attribute
/// turned on spells `Plan.md` and `plan.md` as two different files, and
/// folding them would let a watch register — or a tab open — the wrong one.
/// The case-insensitive comparison on Windows is a fallback, used only when
/// no exact spelling matches ([`paths_match`], [`path_is_covered`]).
///
/// The result is a path: a caller registers or opens with it as it was given,
/// minus a Windows verbatim prefix.
pub fn normalize_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        if let Some(Component::Prefix(prefix)) = path.components().next() {
            match prefix.kind() {
                Prefix::VerbatimDisk(letter) => {
                    let mut plain = PathBuf::from(format!("{}:\\", letter as char));
                    for component in path.components().skip(2) {
                        plain.push(component.as_os_str());
                    }
                    plain
                }
                Prefix::VerbatimUNC(server, share) => {
                    let mut plain = PathBuf::from(format!(
                        r"\\{}\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ));
                    for component in path.components().skip(2) {
                        plain.push(component.as_os_str());
                    }
                    plain
                }
                _ => path.to_path_buf(),
            }
        } else {
            path.to_path_buf()
        }
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn fold_case(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// Whether two paths name the same entry: the exact spelling first and, on
/// Windows only when the exact spelling does not match, the case-insensitive
/// one. `Plan.md` and `plan.md` in a case-sensitive directory stay two
/// paths.
pub fn paths_match(left: &Path, right: &Path) -> bool {
    same_path(&normalize_path(left), &normalize_path(right))
}

/// Whether `path` is `root` or below it, with the same exact-first,
/// Windows-only case-insensitive fallback as [`paths_match`].
pub fn path_is_covered(path: &Path, root: &Path) -> bool {
    covered_by(&normalize_path(path), &normalize_path(root))
}

fn covered_by(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    #[cfg(windows)]
    {
        fold_case(path).starts_with(fold_case(root))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(windows)]
    {
        fold_case(left) == fold_case(right)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The key a document is matched by: the normalized spelling of its path
/// and, when it was opened through a symbolic link, the normalized spelling
/// of the resolved target.
///
/// The key keeps the spelling's case. A caller matches the exact spelling
/// first and only falls back to [`Self::reads_ignoring_case`] when no
/// identity matches exactly, so a directory that keeps `Plan.md` and
/// `plan.md` apart opens, watches and reloads both.
///
/// A key is a path as the document was opened (minus a Windows verbatim
/// prefix), never a folded comparison value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchIdentity {
    path: PathBuf,
    target: Option<PathBuf>,
}

impl WatchIdentity {
    /// The identity of the document at `path`, opened through
    /// `resolved_target` when it is a symbolic link.
    pub fn of(path: &Path, resolved_target: Option<&Path>) -> Self {
        Self {
            path: normalize_path(path),
            target: resolved_target.map(normalize_path),
        }
    }

    /// The normalized spelling of the document's own path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The normalized spelling of the resolved target, when the document was
    /// opened through a symbolic link.
    pub fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    /// Whether `key` names the document's path or, for a symbolic link, its
    /// resolved target, with the exact spelling.
    pub fn reads_exactly(&self, key: &Path) -> bool {
        self.path == key || self.target.as_deref() == Some(key)
    }

    /// The Windows case-insensitive match, used only when no identity
    /// matches `key` exactly. On other platforms this is
    /// [`Self::reads_exactly`].
    pub fn reads_ignoring_case(&self, key: &Path) -> bool {
        if self.reads_exactly(key) {
            return true;
        }
        #[cfg(windows)]
        {
            same_path(&self.path, key)
                || self
                    .target
                    .as_deref()
                    .is_some_and(|target| same_path(target, key))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

impl Watcher {
    /// Start with nothing watched.
    ///
    /// A workspace root arrives with [`Self::set_root`], and a document opened
    /// outside any workspace still needs its own directory watched, which is
    /// what [`Self::sync_document_directories`] is for. A watcher that
    /// insisted on a root would have to invent one — the user's home, the
    /// current directory — and a recursive watch of an invented root is not a
    /// bounded watch.
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
    pub fn new() -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
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
        let (commands, command_rx) = channel();
        let (problems, problem_rx) = channel();
        let applied = Arc::new(Mutex::new(AppliedWatches::default()));
        let worker = WatchWorker {
            debouncer,
            root: None,
            canonical_root: None,
            desired_directories: HashSet::new(),
            document_directories: HashSet::new(),
            applied: applied.clone(),
            problems,
        };
        std::thread::Builder::new()
            .name("markturbo-watcher".to_owned())
            .spawn(move || worker.run(command_rx))?;
        Ok(Self {
            commands,
            rx,
            problems: problem_rx,
            applied,
            context_paths: HashSet::new(),
        })
    }

    /// Watch `root` recursively, replacing the previous root's watch.
    ///
    /// The root's recursive watch is the only recursive one: a document
    /// directory outside it is watched by
    /// [`Self::sync_document_directories`] non-recursively, and a document
    /// directory that the new root now covers has its separate watch removed.
    /// The registration runs on the worker: this returns once the command is
    /// queued.
    pub fn set_root(&mut self, root: &Path) -> anyhow::Result<()> {
        let root = normalize_path(root);
        if self.root() == Some(root.clone()) {
            return Ok(());
        }
        self.commands.send(Command::SetRoot(root))?;
        Ok(())
    }

    /// Match the non-recursive watches to the parents of open documents.
    ///
    /// Save As and Open File can give a document a path outside the workspace
    /// tree. Those parents remain bounded by the open tab set and disappear
    /// when the last document using them closes or moves. The diff and the
    /// registrations run on the worker: this returns once the set is queued.
    pub fn sync_document_directories(
        &mut self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> anyhow::Result<()> {
        let desired: HashSet<PathBuf> = directories
            .into_iter()
            .map(|directory| normalize_path(&directory))
            .collect();
        self.commands
            .send(Command::SyncDocumentDirectories(desired))?;
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
        self.context_paths = paths
            .into_iter()
            .map(|path| normalize_path(&path))
            .collect();
    }

    /// Drain pending changes without blocking.
    ///
    /// Returns an empty vec when nothing happened, so this is safe to poll from
    /// a UI tick.
    pub fn poll(&self) -> Vec<Change> {
        let (root, directories) = {
            let applied = self
                .applied
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (applied.root.clone(), applied.document_directories.clone())
        };
        let mut changes = Vec::new();
        while let Ok(events) = self.rx.try_recv() {
            for event in events {
                changes.extend(classify(
                    &event,
                    root.as_deref(),
                    &directories,
                    &self.context_paths,
                ));
            }
        }
        dedup(changes)
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.applied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .root
            .clone()
    }

    /// Take the registration failures the worker recorded, oldest first.
    ///
    /// A caller drains this on its own tick; a failure never blocks the file
    /// operations that continue to work without live watching.
    pub fn drain_problems(&self) -> Vec<String> {
        self.problems.try_iter().collect()
    }

    /// The applied watch generations: the root's and the document
    /// directories'. A change tells the caller to re-derive what the watch
    /// now covers.
    pub fn applied_generations(&self) -> (u64, u64) {
        let applied = self
            .applied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (applied.root_generation, applied.directory_generation)
    }

    /// Wait until the worker applied every earlier command. Test-only: the
    /// production callers never block on registration.
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush(&self) {
        let (done, wait) = channel();
        if self.commands.send(Command::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// The non-recursive document directories watched right now, for a probe
    /// that pins a tab's watch being added and removed.
    #[cfg(any(test, feature = "test-support"))]
    pub fn watched_document_directories(&self) -> Vec<PathBuf> {
        self.flush();
        self.applied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .document_directories
            .iter()
            .cloned()
            .collect()
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
    root: Option<&Path>,
    document_directories: &HashSet<PathBuf>,
    context_paths: &HashSet<PathBuf>,
) -> Vec<Change> {
    use notify::EventKind;

    event
        .paths
        .iter()
        .enumerate()
        .map(|(index, path)| (index, normalize_path(path)))
        .filter(|(_, path)| {
            if context_paths.contains(path.as_path())
                || context_paths.iter().any(|context| same_path(context, path))
            {
                return true;
            }
            let covering = root
                .into_iter()
                .chain(document_directories.iter().map(PathBuf::as_path))
                .filter(|root| covered_by(path, root))
                .max_by_key(|root| root.components().count());
            match covering {
                Some(root) => !is_noise(path, root),
                // No watch of ours covers the path: it arrived from a watch
                // the caller removed between the event and this poll, and
                // there is no root to judge its components against.
                None => true,
            }
        })
        .filter_map(|(index, path)| {
            Some(match event.kind {
                EventKind::Create(_) => Change::Created(path),
                EventKind::Remove(_) => Change::Removed(path),
                EventKind::Modify(notify::event::ModifyKind::Name(rename)) => {
                    // A rename names its two halves; the platform says which
                    // is which. `Both` carries the pair in `[from, to]`
                    // order, so its per-path verdict is positional. Only a
                    // platform that cannot say needs the path probed.
                    match rename {
                        notify::event::RenameMode::To => Change::Created(path),
                        notify::event::RenameMode::From => Change::Removed(path),
                        notify::event::RenameMode::Both if index == 0 => Change::Removed(path),
                        notify::event::RenameMode::Both => Change::Created(path),
                        notify::event::RenameMode::Any | notify::event::RenameMode::Other => {
                            if path.exists() {
                                Change::Created(path)
                            } else {
                                Change::Removed(path)
                            }
                        }
                    }
                }
                EventKind::Modify(_) => Change::Modified(path),
                // Access events say nothing about content.
                _ => return None,
            })
        })
        .collect()
}

// Collapse repeats, keeping the last verdict per path: a save arrives as
// create+modify, while two Windows paths that differ only in case are two
// files and each keeps its own verdict.
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
    use std::io::Write as _;

    fn event(kind: notify::EventKind, paths: &[PathBuf]) -> DebouncedEvent {
        let event = paths.iter().fold(notify::Event::new(kind), |event, path| {
            event.add_path(path.clone())
        });
        DebouncedEvent::new(event, std::time::Instant::now())
    }

    /// The watcher shape every rooted test had before the root became
    /// optional: no watches, then one recursive root watch. The flush waits
    /// for the worker to apply it, so a write that follows cannot race the
    /// registration.
    fn rooted(root: &Path) -> Watcher {
        let mut watcher = Watcher::new().unwrap();
        watcher.set_root(root).unwrap();
        watcher.flush();
        watcher
    }

    #[test]
    fn context_marker_creation_and_removal_bypass_noise_filtering() {
        use notify::EventKind;
        use notify::event::{CreateKind, RemoveKind};

        let dir = tempfile::tempdir().unwrap();
        let marker = normalize_path(&dir.path().join(".git"));
        let mut watcher = rooted(dir.path());
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
        let from = normalize_path(&dir.path().join(".git"));
        let to = normalize_path(&dir.path().join("nested/.git"));
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::create_dir(&from).unwrap();
        let context_paths = HashSet::from([from.clone(), to.clone()]);
        let rename = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[from.clone(), to.clone()],
        );

        std::fs::rename(&from, &to).unwrap();
        assert_eq!(
            classify(&rename, Some(dir.path()), &HashSet::new(), &context_paths),
            vec![Change::Removed(from.clone()), Change::Created(to.clone())]
        );
        std::fs::rename(&to, &from).unwrap();
        let reverse = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[to.clone(), from.clone()],
        );
        assert_eq!(
            classify(&reverse, Some(dir.path()), &HashSet::new(), &context_paths),
            vec![Change::Removed(to), Change::Created(from)]
        );
    }

    #[test]
    fn context_paths_do_not_allow_descendants_or_unregistered_noise() {
        use notify::EventKind;
        use notify::event::ModifyKind;

        let dir = tempfile::tempdir().unwrap();
        let marker = normalize_path(&dir.path().join(".git"));
        let candidate = normalize_path(&dir.path().join("node_modules/AGENTS.md"));
        let real = normalize_path(&dir.path().join("real.md"));
        let changed = event(
            EventKind::Modify(ModifyKind::Any),
            &[
                marker.clone(),
                marker.join("HEAD"),
                normalize_path(&dir.path().join("nested/.git")),
                candidate.clone(),
                normalize_path(&dir.path().join("node_modules/pkg.md")),
                normalize_path(&dir.path().join(".venv/AGENTS.md")),
                real.clone(),
            ],
        );
        let mut watcher = rooted(dir.path());
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

    /// Receive a matching change for the exact path from the pre-write subscription.
    fn receive_change(
        watcher: &Watcher,
        path: &Path,
        matches_change: impl Fn(&Change) -> bool,
    ) -> Change {
        let path = normalize_path(path);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        loop {
            let events = watcher
                .rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "expected filesystem notification before the deadline: {error:?}; \
                         observed event kinds and exact path matches: {observed:?}"
                    )
                });
            observed.extend(events.iter().map(|event| {
                (
                    event.kind,
                    event.paths.iter().any(|event_path| event_path == &path),
                )
            }));
            let changes = dedup(
                events
                    .iter()
                    .flat_map(|event| {
                        let applied = watcher
                            .applied
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        classify(
                            event,
                            applied.root.as_deref(),
                            &applied.document_directories,
                            &watcher.context_paths,
                        )
                    })
                    .collect(),
            );
            if let Some(found) = changes
                .into_iter()
                .find(|change| change.path() == path.as_path() && matches_change(change))
            {
                return found;
            }
        }
    }

    #[test]
    fn detects_an_external_modification() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = normalize_path(&root).join("a.md");

        let watcher = rooted(&root);
        std::fs::write(&path, "one\n").unwrap();
        let created = Change::Created(path.clone());
        assert_eq!(
            receive_change(&watcher, &path, |change| matches!(
                change,
                Change::Created(_)
            )),
            created
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"one\n");

        // A fresh subscription excludes queued fixture notifications. Native
        // FSEvents kinds are hints: a content change can still include Created.
        drop(watcher);
        let watcher = rooted(&root);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all(b"two\n")
            .unwrap();

        let changed = receive_change(&watcher, &path, |change| {
            matches!(change, Change::Created(_) | Change::Modified(_))
        });
        assert_eq!(changed.path(), path);
        assert_eq!(std::fs::read(changed.path()).unwrap(), b"two\n");
    }

    #[test]
    fn detects_a_change_in_an_added_directory_outside_the_primary_root() {
        let primary = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let primary_root = primary.path().canonicalize().unwrap();
        let external_root = normalize_path(&external.path().canonicalize().unwrap());
        let path = external_root.join("saved-as.md");

        let mut watcher = rooted(&primary_root);
        watcher
            .sync_document_directories([external_root.clone()])
            .unwrap();
        watcher.flush();
        std::fs::write(&path, "one\n").unwrap();
        let created = Change::Created(path.clone());
        assert_eq!(
            receive_change(&watcher, &path, |change| matches!(
                change,
                Change::Created(_)
            )),
            created
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"one\n");

        drop(watcher);
        let mut watcher = rooted(&primary_root);
        watcher.sync_document_directories([external_root]).unwrap();
        watcher.flush();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all(b"two\n")
            .unwrap();

        let changed = receive_change(&watcher, &path, |change| {
            matches!(change, Change::Created(_) | Change::Modified(_))
        });
        assert_eq!(changed.path(), path);
        assert_eq!(std::fs::read(changed.path()).unwrap(), b"two\n");
    }

    #[test]
    fn content_modification_events_keep_the_exact_modified_classification() {
        use notify::EventKind;
        use notify::event::{DataChange, ModifyKind};

        let root = Path::new("/w");
        let path = root.join("a.md");
        let changed = event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            std::slice::from_ref(&path),
        );
        assert_eq!(
            classify(&changed, Some(root), &HashSet::new(), &HashSet::new()),
            vec![Change::Modified(path)]
        );
    }

    #[test]
    fn syncing_document_directories_drops_roots_no_open_document_uses() {
        let primary = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut watcher = rooted(primary.path());

        watcher
            .sync_document_directories([first.path().to_path_buf(), second.path().to_path_buf()])
            .unwrap();
        assert_eq!(watcher.watched_document_directories().len(), 2);

        watcher
            .sync_document_directories([second.path().to_path_buf()])
            .unwrap();
        let watched = watcher.watched_document_directories();
        assert_eq!(watched.len(), 1);
        assert!(watched.contains(&normalize_path(second.path())));
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
            classify(&created, Some(root), &HashSet::new(), &HashSet::new()),
            vec![Change::Created(real)]
        );
    }

    #[test]
    fn poll_is_non_blocking_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = rooted(dir.path());
        let start = std::time::Instant::now();
        assert!(watcher.poll().is_empty());
        assert!(start.elapsed() < Duration::from_millis(100), "poll blocked");
    }

    #[test]
    fn a_rootless_watcher_reports_a_document_directory_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize_path(&dir.path().canonicalize().unwrap());
        let path = root.join("dropped.md");

        let mut watcher = Watcher::new().unwrap();
        assert_eq!(watcher.root(), None);
        watcher.sync_document_directories([root.clone()]).unwrap();
        watcher.flush();
        std::fs::write(&path, "one\n").unwrap();
        assert_eq!(
            receive_change(&watcher, &path, |change| matches!(
                change,
                Change::Created(_)
            )),
            Change::Created(path.clone())
        );
    }

    #[test]
    fn setting_a_new_root_moves_the_recursive_watch_and_drops_covered_directories() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_root = first.path().canonicalize().unwrap();
        let second_root = normalize_path(&second.path().canonicalize().unwrap());
        let inner = second_root.join("notes");
        std::fs::create_dir(&inner).unwrap();

        let mut watcher = rooted(&first_root);
        watcher.sync_document_directories([inner.clone()]).unwrap();
        assert!(watcher.watched_document_directories().contains(&inner));

        watcher.set_root(&second_root).unwrap();
        watcher.flush();
        assert_eq!(watcher.root(), Some(second_root.clone()));
        assert!(
            !watcher.watched_document_directories().contains(&inner),
            "a directory the new root covers keeps no separate watch"
        );

        let path = inner.join("a.md");
        std::fs::write(&path, "one\n").unwrap();
        assert_eq!(
            receive_change(&watcher, &path, |change| matches!(
                change,
                Change::Created(_)
            )),
            Change::Created(path.clone())
        );
    }

    #[test]
    fn a_canonical_spelling_of_a_rooted_directory_is_not_watched_twice() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let mut watcher = rooted(root.path());

        // `canonicalize` on Windows yields the verbatim (`\\?\C:\…`) spelling;
        // the root was set from the user's own spelling.
        watcher
            .sync_document_directories([canonical.clone()])
            .unwrap();
        assert!(
            watcher.watched_document_directories().is_empty(),
            "one directory must not hold a recursive and a non-recursive watch"
        );
    }

    #[test]
    fn a_path_under_no_watch_is_kept() {
        let mut watcher = Watcher::new().unwrap();
        let (tx, rx) = channel();
        watcher.rx = rx;
        let path = PathBuf::from("/somewhere/else/a.md");
        tx.send(vec![event(
            notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            std::slice::from_ref(&path),
        )])
        .unwrap();
        assert_eq!(watcher.poll(), vec![Change::Modified(path)]);
    }

    #[test]
    fn setting_a_root_that_cannot_be_watched_is_a_problem_the_callers_drain() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-folder");
        let mut watcher = Watcher::new().unwrap();
        watcher.set_root(&missing).unwrap();
        watcher.flush();
        assert!(
            watcher
                .drain_problems()
                .iter()
                .any(|problem| problem.contains("watch the workspace")),
            "a failed root watch is reported to the caller"
        );
        assert_eq!(
            watcher.root(),
            None,
            "a root that could not be watched is never kept"
        );
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
        let watcher = rooted(dir.path());
        let elapsed = start.elapsed();
        drop(watcher);

        assert!(
            elapsed < Duration::from_millis(500),
            "registering the watch took {elapsed:?}; it must not scan the tree"
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_windows_paths_normalize_to_the_plain_spelling_keeping_case() {
        assert_eq!(
            normalize_path(Path::new(r"\\?\C:\notes\plan.md")),
            PathBuf::from(r"C:\notes\plan.md")
        );
        assert_eq!(
            normalize_path(Path::new(r"\\?\UNC\server\share\Plan.MD")),
            PathBuf::from(r"\\server\share\Plan.MD")
        );
        assert_eq!(
            normalize_path(Path::new(r"C:\Notes\Plan.MD")),
            PathBuf::from(r"C:\Notes\Plan.MD")
        );
        assert_eq!(
            normalize_path(Path::new(r"\\?\Volume{8f2a}\Plan.MD")),
            PathBuf::from(r"\\?\Volume{8f2a}\Plan.MD")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_keys_keep_case_and_match_it_only_after_an_exact_miss() {
        let identity = WatchIdentity::of(
            Path::new(r"C:\Notes\Link.md"),
            Some(Path::new(r"\\?\C:\NOTES\Real.md")),
        );
        assert!(identity.reads_exactly(&normalize_path(Path::new(r"C:\Notes\Link.md"))));
        assert!(identity.reads_exactly(&normalize_path(Path::new(r"C:\NOTES\Real.md"))));
        assert!(!identity.reads_exactly(&normalize_path(Path::new(r"c:\notes\link.md"))));
        assert!(identity.reads_ignoring_case(&normalize_path(Path::new(r"c:\notes\link.MD"))));
        assert!(identity.reads_ignoring_case(&normalize_path(Path::new(r"C:\notes\REAL.md"))));
        assert!(!identity.reads_ignoring_case(&normalize_path(Path::new(r"c:\notes\other.md"))));
    }

    #[cfg(windows)]
    #[test]
    fn two_names_that_differ_only_in_case_are_two_documents() {
        let upper = WatchIdentity::of(Path::new(r"C:\Notes\Plan.md"), None);
        let lower = WatchIdentity::of(Path::new(r"C:\Notes\plan.md"), None);
        assert_ne!(upper, lower, "a case-sensitive directory holds two files");
        assert!(upper.reads_exactly(&normalize_path(Path::new(r"C:\Notes\Plan.md"))));
        assert!(!upper.reads_exactly(&normalize_path(Path::new(r"C:\Notes\plan.md"))));
        assert!(lower.reads_exactly(&normalize_path(Path::new(r"C:\Notes\plan.md"))));
    }

    #[test]
    fn an_exact_match_wins_over_a_case_insensitive_one_when_both_would_match() {
        let upper = WatchIdentity::of(Path::new("Plan.md"), None);
        let lower = WatchIdentity::of(Path::new("plan.md"), None);
        let identities = [upper, lower];
        let key = normalize_path(Path::new("Plan.md"));
        assert_eq!(
            identities
                .iter()
                .position(|identity| identity.reads_exactly(&key)),
            Some(0),
            "the exact spelling names the first document only"
        );
        let miss = normalize_path(Path::new("PLAN.MD"));
        assert!(
            !identities
                .iter()
                .any(|identity| identity.reads_exactly(&miss)),
            "no document spells the miss exactly"
        );
        #[cfg(windows)]
        assert!(
            identities
                .iter()
                .any(|identity| identity.reads_ignoring_case(&miss)),
            "the miss falls back to the case-insensitive match on Windows"
        );
    }

    #[test]
    fn paths_match_exactly_first_and_cover_prefixes_the_same_way() {
        assert!(paths_match(
            Path::new("/notes/plan.md"),
            Path::new("/notes/plan.md")
        ));
        let case_only = paths_match(Path::new("/notes/plan.md"), Path::new("/notes/plan.MD"));
        #[cfg(windows)]
        assert!(
            case_only,
            "Windows falls back to the case-insensitive match"
        );
        #[cfg(not(windows))]
        assert!(!case_only, "Unix keeps its case");
        assert!(path_is_covered(
            Path::new("/notes/a/b.md"),
            Path::new("/notes")
        ));
        assert!(!path_is_covered(
            Path::new("/customize/a.md"),
            Path::new("/custom")
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_plain_path_normalizes_to_itself() {
        let path = Path::new("/notes/plan.md");
        assert_eq!(normalize_path(path), path);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_keys_keep_their_case_through_a_link_and_its_target() {
        let identity = WatchIdentity::of(
            Path::new("/notes/Link.md"),
            Some(Path::new("/Notes/Real.md")),
        );
        assert!(identity.reads_exactly(Path::new("/notes/Link.md")));
        assert!(identity.reads_exactly(Path::new("/Notes/Real.md")));
        assert!(!identity.reads_exactly(Path::new("/notes/link.md")));
        assert!(!identity.reads_ignoring_case(Path::new("/notes/link.md")));
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
    fn dedup_keeps_paths_that_differ_only_in_case_separate() {
        let upper = PathBuf::from("Plan.md");
        let lower = PathBuf::from("plan.md");
        let out = dedup(vec![
            Change::Modified(upper.clone()),
            Change::Modified(lower.clone()),
        ]);
        assert_eq!(out, vec![Change::Modified(upper), Change::Modified(lower)]);
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
