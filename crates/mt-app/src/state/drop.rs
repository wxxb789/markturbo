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
    /// Classify the items a drag offers: by the file system for local paths,
    /// by the name otherwise.
    pub fn of(uris: &[String]) -> Self {
        let mut confirmed = 0usize;
        let mut single: Option<SharedString> = None;
        let mut unknown = false;
        let mut unsupported = 0usize;
        for uri in uris {
            match offered_kind(uri) {
                Offered::Openable(name) => {
                    confirmed += 1;
                    single = Some(name);
                }
                Offered::Unknown => unknown = true,
                Offered::Unsupported => unsupported += 1,
            }
        }
        match (confirmed, unknown, unsupported) {
            (0, _, 0) => DropOffer::Unknown,
            (0, false, _) => DropOffer::Unsupported,
            (0, true, _) => DropOffer::Unknown,
            (1, ..) => DropOffer::One {
                name: single.expect("one confirmed item has a name"),
            },
            (count, ..) => DropOffer::Many { count },
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

/// What one offered item's name and, when it names a local path, the file
/// system confirm.
///
/// A name alone never confirms a file: on a file drag the event carries names
/// and inferred MIME types only, and a folder named `archive.v1` gets a MIME
/// type from that name. A path the file system answers for does confirm its
/// kind; without that answer nothing is called unsupported.
enum Offered {
    /// A supported document, or a folder, with the name to show.
    Openable(SharedString),
    /// No reliable answer: it might be a folder, or nothing the app can read.
    Unknown,
    /// A regular file whose type the app's document types reject.
    Unsupported,
}

fn offered_kind(uri: &str) -> Offered {
    if let Some(path) = file_path_of_uri(uri)
        && let Ok(metadata) = std::fs::metadata(&path)
    {
        let Some(name) = path_name(&path) else {
            return Offered::Unknown;
        };
        if metadata.is_dir() {
            return Offered::Openable(SharedString::from(name));
        }
        if metadata.is_file() {
            return if DocType::of(&path).is_document() {
                Offered::Openable(SharedString::from(name))
            } else {
                Offered::Unsupported
            };
        }
        return Offered::Unknown;
    }
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
    /// A drag entered: offer what its names confirm. A card, Settings or
    /// another surface that replaces the workspace owns the interaction, so a
    /// drag behind it offers nothing.
    pub fn drop_hover(&self, uris: &[String]) {
        if self.drop_blocked() {
            return;
        }
        self.drop_offer.set(Some(DropOffer::of(uris)));
    }

    /// The drag left without dropping: the affordance clears.
    pub fn drop_leave(&self) {
        self.drop_offer.set(None);
    }

    /// A drop: open its documents in order in one task, make the last folder
    /// the workspace, and report the skipped count once, last, naming no
    /// paths. The folder's request token is claimed here, before the task's
    /// first await, so a workspace the user opens while the documents load is
    /// never overwritten by the older drop; the same check closes the task if
    /// a card or Settings is up by the time it commits.
    pub fn drop_files(&self, uris: &[String]) {
        self.drop_offer.set(None);
        if self.drop_blocked() {
            return;
        }
        let language = self.settings.get().language;
        let plan = plan_drop(uris);
        let folder = plan.folder.map(|folder| (folder, self.begin_folder_open()));
        let state = self.clone();
        self.spawn_task(async move {
            let mut skipped = state.open_dropped_documents(plan.documents).await;
            skipped += plan.skipped;
            if let Some((folder, request)) = folder {
                if state.drop_blocked() {
                    return;
                }
                state.finish_folder_open(folder, request, true).await;
            }
            if skipped > 0 {
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
    /// file end in one tab.
    async fn open_dropped_documents(&self, paths: Vec<PathBuf>) -> usize {
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
                    io::load(&path).map(Some)
                })
                .await
                {
                    Ok(Ok(Some(loaded))) => loaded,
                    Ok(Ok(None)) => {
                        skipped += 1;
                        continue;
                    }
                    Ok(Err(error)) => {
                        self.notify_failure(crate::i18n::Key::OpenFailed, &error);
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
            self.notify_named(crate::i18n::Key::Opened, &opened);
        }
        self.focus_editor();
        skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supported_document_name_is_confirmed() {
        assert_eq!(
            DropOffer::of(&["file:///w/plan.md".to_owned()]),
            DropOffer::One {
                name: SharedString::from("plan.md")
            }
        );
        assert_eq!(
            DropOffer::of(&["file:///w/a.md".to_owned(), "file:///w/b.txt".to_owned()]),
            DropOffer::Many { count: 2 }
        );
    }

    #[test]
    fn a_folder_marked_by_a_trailing_slash_is_confirmed_and_an_untyped_name_is_not() {
        assert_eq!(
            DropOffer::of(&["file:///w/notes/".to_owned()]),
            DropOffer::One {
                name: SharedString::from("notes")
            }
        );
        // No extension and no document name: it may be a folder the event did
        // not type, so it is neither offered as openable nor called
        // unsupported. (`README` would be a document name, not this case.)
        assert_eq!(
            DropOffer::of(&["file:///w/binary-blob".to_owned()]),
            DropOffer::Unknown
        );
        assert_eq!(DropOffer::of(&[]), DropOffer::Unknown);
    }

    #[test]
    fn only_a_confirmed_file_reads_as_unsupported() {
        let directory = tempfile::tempdir().unwrap();
        let picture = directory.path().join("photo.png");
        std::fs::write(&picture, b"\x89PNG").unwrap();
        let document = directory.path().join("plan.md");
        std::fs::write(&document, "# plan\n").unwrap();
        let picture_uri = url::Url::from_file_path(&picture).unwrap().to_string();
        let document_uri = url::Url::from_file_path(&document).unwrap().to_string();

        assert_eq!(
            DropOffer::of(std::slice::from_ref(&picture_uri)),
            DropOffer::Unsupported
        );
        assert_eq!(
            DropOffer::of(&[picture_uri.clone(), "file:///w/binary-blob".to_owned()]),
            DropOffer::Unknown
        );
        assert_eq!(
            DropOffer::of(&[picture_uri, document_uri]),
            DropOffer::One {
                name: SharedString::from("plan.md")
            }
        );
    }

    #[test]
    fn a_folder_named_like_a_file_is_confirmed_openable() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("archive.v1");
        std::fs::create_dir(&folder).unwrap();
        let uri = url::Url::from_directory_path(&folder).unwrap().to_string();
        // A Windows folder URI does not have to end in a slash: the kind has
        // to come from the file system, not the name.
        assert_eq!(
            DropOffer::of(&[uri.trim_end_matches('/').to_owned()]),
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
            DropOffer::of(&[uri]),
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
        assert_eq!(DropOffer::of(&[uri]), DropOffer::Unknown);
    }
}
