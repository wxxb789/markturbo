//! Files and folders dropped on the window: what a hover offers, what a drop
//! opens, and what it counts as skipped.
//!
//! The event carries names (absolute `file:` URIs) and inferred MIME types,
//! nothing else. Only what a name confirms is acted on: a name the app's
//! document types accept, or a folder the event marked as one with a trailing
//! slash. A name with no extension might be a folder the event did not type,
//! so it is never treated as an unsupported file.

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
    /// Classify the items a drag offers, by their names alone.
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

/// What one offered item's name confirms.
enum Offered {
    /// A supported document, or a folder the event marked with a trailing
    /// slash, with the name to show.
    Openable(SharedString),
    /// No extension (or no name): it might be a folder the event did not
    /// type, so nothing is confirmed and nothing is refused.
    Unknown,
    /// A name whose type the app's document types reject.
    Unsupported,
}

fn offered_kind(uri: &str) -> Offered {
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
    if has_extension(&name) {
        Offered::Unsupported
    } else {
        Offered::Unknown
    }
}

/// The last path segment of `uri`, percent-decoded when it parses as a URL.
fn uri_name(uri: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(uri)
        && let Some(mut segments) = url.path_segments()
        && let Some(name) = segments.rfind(|segment| !segment.is_empty())
    {
        return Some(name.to_owned());
    }
    let trimmed = uri.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// A trailing extension: `.env` is a name, not an extension, as the MIME
/// table treats it.
fn has_extension(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
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
        if path.is_dir() {
            plan.folder = Some(path);
        } else if DocType::of(&path).is_document() {
            plan.documents.push(path);
        } else {
            plan.skipped += 1;
        }
    }
    plan
}

impl AppState {
    /// A drag entered: offer what its names confirm.
    pub fn drop_hover(&self, uris: &[String]) {
        self.drop_offer.set(Some(DropOffer::of(uris)));
    }

    /// The drag left without dropping: the affordance clears.
    pub fn drop_leave(&self) {
        self.drop_offer.set(None);
    }

    /// A drop: open its documents in order in one task, make the last folder
    /// the workspace, and report the skipped count once, last, naming no
    /// paths.
    pub fn drop_files(&self, uris: &[String]) {
        self.drop_offer.set(None);
        let language = self.settings.get().language;
        let plan = plan_drop(uris);
        let state = self.clone();
        self.spawn_task(async move {
            state.open_dropped_documents(plan.documents).await;
            if let Some(folder) = plan.folder {
                state.open_folder_now(folder).await;
            }
            if plan.skipped > 0 {
                state.notify_held(SharedString::from(s::skipped_files(language, plan.skipped)));
            }
        });
    }

    /// Open each path in a tab, or focus the tab already showing it. One task
    /// in order, so the last one ends up active and its load never races
    /// another's.
    async fn open_dropped_documents(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        for path in paths {
            if let Some(tab) = self.tabs.get().iter().find(|tab| tab.path == path) {
                self.set_active(tab.id);
                continue;
            }
            let loaded = {
                let path = path.clone();
                match blocking(move || {
                    #[cfg(any(test, feature = "test-vault"))]
                    crate::test_vault::wait_for_read(&path);
                    io::load(&path)
                })
                .await
                {
                    Ok(Ok(loaded)) => loaded,
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
            let opened = loaded.path.clone();
            self.add_tab(loaded);
            self.record_usage(UsageEvent::ArtifactOpened);
            self.notify_named(crate::i18n::Key::Opened, &opened);
        }
        self.focus_editor();
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
    fn only_all_confirmed_unsupported_types_read_as_unsupported() {
        assert_eq!(
            DropOffer::of(&["file:///w/photo.png".to_owned()]),
            DropOffer::Unsupported
        );
        assert_eq!(
            DropOffer::of(&[
                "file:///w/photo.png".to_owned(),
                "file:///w/binary-blob".to_owned()
            ]),
            DropOffer::Unknown
        );
        assert_eq!(
            DropOffer::of(&[
                "file:///w/photo.png".to_owned(),
                "file:///w/plan.md".to_owned()
            ]),
            DropOffer::One {
                name: SharedString::from("plan.md")
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
}
