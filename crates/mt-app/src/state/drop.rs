//! Files and folders dropped on the window: what a hover offers, what a drop
//! opens, and what it counts as skipped.
//!
//! The event carries names (absolute `file:` URIs) and inferred MIME types.
//! When a name is a local path the file system answers for, its kind and
//! document type decide; otherwise only the name can: a name the app's
//! document types accept, or a folder the event marked as one with a trailing
//! slash. A name that confirms neither may still be a folder, so it is never
//! treated as an unsupported file.

use super::*;
use mt_core::DocType;

/// What a drag hovering over the window offers to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropOffer {
    /// Exactly one item the app can confirm it will open.
    One { name: SharedString },
    /// Several confirmed items.
    Many { count: usize },
    /// Nothing can be confirmed (no names, or only inferred types): the drop
    /// may still open a folder or a document.
    Unknown,
    /// Every item is a confirmed file of a type the app cannot open.
    Unsupported,
}

impl DropOffer {
    /// Classify the items a drag offers from its names and the event's types
    /// only: a drag callback never touches the file system. The event's MIME
    /// types are inferred from the name's extension, so they never prove an
    /// item is a file; only a document name or a folder the event marked
    /// confirms anything.
    pub fn of(uris: &[String], mime_types: &[String]) -> Self {
        let mut confirmed = 0usize;
        let mut single: Option<SharedString> = None;
        for (index, uri) in uris.iter().enumerate() {
            let mime_type = mime_types
                .get(index)
                .map(String::as_str)
                .unwrap_or_default();
            if let Offered::Openable(name) = offered_kind(uri, mime_type) {
                confirmed += 1;
                single = Some(name);
            }
        }
        match confirmed {
            0 => DropOffer::Unknown,
            1 => DropOffer::One {
                name: single.expect("one confirmed item has a name"),
            },
            count => DropOffer::Many { count },
        }
    }

    /// The line the drop screen shows.
    pub fn title(&self, language: Language) -> String {
        match self {
            DropOffer::One { name } => match language {
                Language::English => format!("Open {name}"),
                Language::Chinese => format!("打开 {name}"),
            },
            DropOffer::Many { count } => match language {
                Language::English => format!("Open {count} files"),
                Language::Chinese => format!("打开 {count} 个文件"),
            },
            DropOffer::Unknown => match language {
                Language::English => "Drop to open".to_owned(),
                Language::Chinese => "拖放以打开".to_owned(),
            },
            DropOffer::Unsupported => match language {
                Language::English => "Can't open this file type".to_owned(),
                Language::Chinese => "无法打开此文件类型".to_owned(),
            },
        }
    }

    /// The quieter line under the title: what a drop does with what it takes.
    pub fn hint(language: Language) -> &'static str {
        match language {
            Language::English => "Opens documents in tabs · a folder becomes the workspace",
            Language::Chinese => "在标签页中打开文档 · 文件夹将成为工作区",
        }
    }
}

/// What one offered item's name and the event's type say about it.
///
/// A document name and a trailing slash need no file system to confirm. An
/// inferred type proves nothing — nsh reads it from the name's extension, so
/// a folder named `archive.png` carries one — and an item that might be a
/// folder is never called unsupported.
enum Offered {
    /// A supported document, or a folder the event typed, with the name to
    /// show.
    Openable(SharedString),
    /// No reliable answer: it might be a folder, or nothing the app can read.
    Unknown,
}

fn offered_kind(uri: &str, _mime_type: &str) -> Offered {
    if uri.ends_with('/') || uri.ends_with('\\') {
        return match uri_name(uri) {
            Some(name) => Offered::Openable(SharedString::from(name)),
            None => Offered::Unknown,
        };
    }
    let Some(name) = uri_name(uri) else {
        return Offered::Unknown;
    };
    if DocType::of(Path::new(&name)).is_document() {
        return Offered::Openable(SharedString::from(name));
    }
    Offered::Unknown
}

fn path_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}

/// The last path segment of `uri`, decoded when the URI names a local file.
fn uri_name(uri: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(uri)
        && let Ok(path) = url.to_file_path()
        && let Some(name) = path_name(&path)
    {
        return Some(name);
    }
    let trimmed = uri.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// The path behind a dropped `file:` URI; anything else is nothing the app
/// may open.
pub(crate) fn file_path_of_uri(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

/// What one drop asks the app to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DropPlan {
    pub(crate) documents: Vec<PathBuf>,
    /// The last folder in the drop becomes the workspace; a workspace is
    /// singular, so earlier ones are ignored.
    pub(crate) folder: Option<PathBuf>,
    /// Paths whose type the app cannot open, counted for one notice that
    /// never names them.
    pub(crate) skipped: usize,
}

pub(crate) fn plan_drop(uris: &[String]) -> DropPlan {
    let mut plan = DropPlan::default();
    for uri in uris {
        let Some(path) = file_path_of_uri(uri) else {
            plan.skipped += 1;
            continue;
        };
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => plan.folder = Some(path),
            Ok(metadata) if metadata.is_file() && DocType::of(&path).is_document() => {
                plan.documents.push(path);
            }
            _ => plan.skipped += 1,
        }
    }
    plan
}

impl AppState {
    /// A drag entered: offer what its names and types confirm. A card,
    /// Settings or another surface that replaces the workspace owns the
    /// interaction, so a drag behind it offers nothing.
    pub fn drop_hover(&self, uris: &[String], mime_types: &[String]) {
        if self.drop_blocked() {
            return;
        }
        self.drop_offer.set(Some(DropOffer::of(uris, mime_types)));
    }

    /// The drag left without dropping: the affordance clears.
    pub fn drop_leave(&self) {
        self.drop_offer.set(None);
    }

    /// A drop: plan it off the UI thread, open its documents in order, make
    /// the last folder the workspace, and report the skipped count once,
    /// last, naming no paths. The folder intent in force when the drop
    /// happened is what the commit compares against, so a workspace the user
    /// opens while the plan or the documents are in flight is never
    /// overwritten by the older drop; a card or Settings that goes up
    /// meanwhile closes the task the same way. A superseded drop is quiet:
    /// neither its folder nor its skipped count may replace the newer
    /// workspace's notice.
    pub fn drop_files(&self, uris: &[String]) {
        self.drop_offer.set(None);
        if self.drop_blocked() {
            return;
        }
        let language = self.settings.get().language;
        let folder_started_at = self.folder_request.get();
        let uris = uris.to_vec();
        let state = self.clone();
        self.spawn_task(async move {
            let plan = match blocking(move || plan_drop(&uris)).await {
                Ok(plan) => plan,
                Err(_) => return,
            };
            let mut skipped = state
                .open_dropped_documents(plan.documents, folder_started_at)
                .await;
            skipped += plan.skipped;
            // A folder drop's own notice belongs to the folder commit; a
            // superseded or blocked one publishes nothing, not even the
            // skipped count.
            let mut may_publish_skipped = plan.folder.is_none();
            if let Some(folder) = plan.folder
                && !state.drop_blocked()
                && state.folder_request.get() == folder_started_at
            {
                let request = state.begin_folder_open();
                may_publish_skipped = state.finish_folder_open(folder, request, true).await;
            }
            if skipped > 0 && may_publish_skipped {
                state.notify_held(SharedString::from(s::skipped_files(language, skipped)));
            }
        });
    }

    /// Whether a card, Settings, or another surface that replaces the
    /// workspace is up: a drag must neither offer nor commit behind it.
    pub(crate) fn drop_blocked(&self) -> bool {
        crate::ui::consent::is_open(self)
            || crate::ui::notices::guard_is_open(self)
            || crate::ui::settings::is_open(self)
    }

    /// Open each path in a tab, or focus the tab already showing it. One task
    /// in order, so the last one ends up active and its load never races
    /// another's. Each path is confirmed a regular file at the load boundary,
    /// so a FIFO or a path that changed kind is skipped, never read; the
    /// tab lookup is repeated after the load, so two concurrent drops of one
    /// file end in one tab. Every `Opened …` notice is bound to
    /// `workspace_request`, the folder request in force when the drop
    /// happened: a drop the user superseded by opening another workspace
    /// publishes nothing.
    async fn open_dropped_documents(&self, paths: Vec<PathBuf>, workspace_request: u64) -> usize {
        let mut skipped = 0;
        for path in paths {
            if self.drop_blocked() {
                break;
            }
            if let Some(tab) = self.tab_showing(&path) {
                self.set_active(tab.id);
                continue;
            }
            let loaded = {
                let path = path.clone();
                match blocking(move || {
                    #[cfg(any(test, feature = "test-vault"))]
                    crate::test_vault::wait_for_read(&path);
                    if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                        return Ok(None);
                    }
                    let loaded = io::load(&path).map(Some);
                    #[cfg(any(test, feature = "test-vault"))]
                    crate::test_vault::wait_for_commit(&path);
                    loaded
                })
                .await
                {
                    Ok(Ok(Some(loaded))) => loaded,
                    Ok(Ok(None)) => {
                        skipped += 1;
                        continue;
                    }
                    Ok(Err(error)) => {
                        if error.kind() == std::io::ErrorKind::InvalidInput {
                            // The opened handle refused a non-regular file: a
                            // FIFO or device counts as skipped, never read.
                            skipped += 1;
                        } else {
                            self.notify_failure(crate::i18n::Key::OpenFailed, &error);
                        }
                        continue;
                    }
                    Err(error) => {
                        self.notify_failure(crate::i18n::Key::OpenFailed, &error);
                        continue;
                    }
                }
            };
            if self.drop_blocked() {
                break;
            }
            let opened = loaded.path.clone();
            if let Some(tab) = self.tab_showing(&opened) {
                self.set_active(tab.id);
                continue;
            }
            self.add_tab(loaded);
            self.record_usage(UsageEvent::ArtifactOpened);
            if self.folder_request.get() == workspace_request {
                self.notify_named(crate::i18n::Key::Opened, &opened);
            }
        }
        self.focus_editor();
        skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offer the app computes for these URIs, with the same types the
    /// drag event carries for their names.
    fn offer(uris: &[&str]) -> DropOffer {
        let uris: Vec<String> = uris.iter().map(|uri| (*uri).to_owned()).collect();
        let mime_types: Vec<String> = uris
            .iter()
            .map(|uri| nsh_core::platform::file_mime_type(uri).to_owned())
            .collect();
        DropOffer::of(&uris, &mime_types)
    }

    #[test]
    fn a_supported_document_name_is_confirmed() {
        assert_eq!(
            offer(&["file:///w/plan.md"]),
            DropOffer::One {
                name: SharedString::from("plan.md")
            }
        );
        assert_eq!(
            offer(&["file:///w/a.md", "file:///w/b.txt"]),
            DropOffer::Many { count: 2 }
        );
    }

    #[test]
    fn a_folder_marked_by_a_trailing_slash_is_confirmed_and_an_untyped_name_is_not() {
        assert_eq!(
            offer(&["file:///w/notes/"]),
            DropOffer::One {
                name: SharedString::from("notes")
            }
        );
        // No extension and no type: it may be a folder the event did not
        // type, so it is neither offered as openable nor called unsupported.
        assert_eq!(offer(&["file:///w/binary-blob"]), DropOffer::Unknown);
        assert_eq!(DropOffer::of(&[], &[]), DropOffer::Unknown);
    }

    #[test]
    fn an_inferred_type_never_proves_an_item_is_a_file() {
        // nsh infers the type from the name's extension, so a folder named
        // `archive.png` carries one too. Nothing inferred may make the hover
        // call an item unsupported; the drop's own metadata settles it.
        assert_eq!(offer(&["file:///w/photo.png"]), DropOffer::Unknown);
        assert_eq!(
            offer(&["file:///w/photo.png", "file:///w/binary-blob"]),
            DropOffer::Unknown
        );
        assert_eq!(
            offer(&["file:///w/photo.png", "file:///w/plan.md"]),
            DropOffer::One {
                name: SharedString::from("plan.md")
            }
        );
        assert_eq!(offer(&["file:///w/archive.png"]), DropOffer::Unknown);
    }

    #[test]
    fn a_folder_name_with_neither_slash_nor_type_is_never_unsupported() {
        assert_eq!(offer(&["file:///w/archive.v1"]), DropOffer::Unknown);
        // The event can still mark it as a folder by its trailing slash.
        assert_eq!(
            offer(&["file:///w/archive.v1/"]),
            DropOffer::One {
                name: SharedString::from("archive.v1")
            }
        );
    }

    #[test]
    fn the_offered_name_is_decoded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("My Plan.md");
        let uri = url::Url::from_file_path(&path).unwrap().to_string();

        assert_eq!(
            offer(&[uri.as_str()]),
            DropOffer::One {
                name: SharedString::from("My Plan.md")
            }
        );
    }

    #[test]
    fn the_titles_follow_the_confirmed_unknown_and_unsupported_cases() {
        let english = Language::English;
        assert_eq!(
            DropOffer::One {
                name: SharedString::from("plan.md")
            }
            .title(english),
            "Open plan.md"
        );
        assert_eq!(DropOffer::Many { count: 3 }.title(english), "Open 3 files");
        assert_eq!(DropOffer::Unknown.title(english), "Drop to open");
        assert_eq!(
            DropOffer::Unsupported.title(english),
            "Can't open this file type"
        );
    }

    #[test]
    fn a_drop_plan_keeps_the_last_folder_and_counts_only_unopenable_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let document = root.join("plan.md");
        std::fs::write(&document, "# plan\n").unwrap();
        let picture = root.join("photo.png");
        std::fs::write(&picture, b"\x89PNG").unwrap();
        let first_folder = root.join("a");
        let second_folder = root.join("b");
        std::fs::create_dir(&first_folder).unwrap();
        std::fs::create_dir(&second_folder).unwrap();

        let plan = plan_drop(&[
            url::Url::from_directory_path(&first_folder)
                .unwrap()
                .to_string(),
            url::Url::from_file_path(&document).unwrap().to_string(),
            url::Url::from_file_path(&picture).unwrap().to_string(),
            url::Url::from_directory_path(&second_folder)
                .unwrap()
                .to_string(),
            "https://example.com/not-a-file".to_owned(),
        ]);
        assert_eq!(plan.documents, vec![document]);
        assert_eq!(plan.folder, Some(second_folder));
        assert_eq!(plan.skipped, 2, "the picture and the non-file URI");
    }

    #[test]
    fn a_plan_skips_a_path_that_is_not_a_regular_file_or_directory() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.md");

        let plan = plan_drop(&[url::Url::from_file_path(&missing).unwrap().to_string()]);

        assert!(plan.documents.is_empty());
        assert_eq!(plan.skipped, 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_plan_skips_a_fifo_with_a_document_name() {
        let directory = tempfile::tempdir().unwrap();
        let fifo = directory.path().join("notes.md");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(
            status.is_ok_and(|status| status.success()),
            "mkfifo must create {fifo:?}"
        );

        let uri = url::Url::from_file_path(&fifo).unwrap().to_string();
        let plan = plan_drop(&[uri.clone()]);
        assert!(plan.documents.is_empty(), "a FIFO is never opened");
        assert_eq!(plan.skipped, 1);
        // The hover path knows only the name: it offers the document, and
        // the drop's own handle check skips the FIFO.
        assert_eq!(
            offer(&[uri.as_str()]),
            DropOffer::One {
                name: SharedString::from("notes.md")
            }
        );
    }
}
