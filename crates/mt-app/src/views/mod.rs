//! GPUI views.
//!
//! Views read the document engine; they never own document semantics. That
//! keeps the engine reusable headless and keeps each view small.

pub mod document;
pub mod explorer;
pub mod harness;
pub mod search;
pub mod workspace;

/// Update an entity from an async task, skipping the update if the `App` is
/// already borrowed rather than panicking.
///
/// `WeakEntity::update` returns `Result`, but the only failure it models is
/// "entity released" — it bottoms out in `AsyncApp::update_entity`, which calls
/// the infallible `AppCell::borrow_mut`. A re-entrant borrow therefore panics
/// before that `Result` is ever produced.
///
/// This is reachable on Windows: WebView2 pumps messages, so the window
/// procedure can re-enter mid-draw with the `App` already mutably borrowed
/// (visible in the log as `deferring re-entrant draw`). `update_window` takes
/// the same borrow through `try_borrow_mut` instead, so it reports the conflict
/// rather than aborting the process.
///
/// Skipping costs one frame; panicking costs the session — and every caller
/// here is either a refresh or a debounced reparse that a later notification
/// repeats anyway.
pub fn try_update<T, R>(
    entity: &gpui::WeakEntity<T>,
    cx: &mut gpui::AsyncApp,
    f: impl FnOnce(&mut T, &mut gpui::Context<T>) -> R,
) -> Option<R>
where
    T: 'static,
{
    use gpui::AppContext as _;

    let entity = entity.upgrade()?;
    cx.with_window(entity.entity_id(), |_, app| app.update_entity(&entity, f))
}

/// How a document is laid out.
///
/// One flat list rather than a mode plus a separate preview choice. The old
/// shape made `Split` mean two different things depending on a second control
/// that only appeared once Split was selected, so the five layouts a user
/// actually picks between were spread across two widgets.
///
/// Exactly one is active at a time, and each names exactly one preview
/// renderer — which is what lets every other question ("does this use the
/// WebView?", "is the editor visible?") be answered from the layout alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The editor alone.
    Source,
    /// GPUI-native rendering: the fast path.
    Native,
    /// WebView rendering: the compatibility path.
    Web,
    /// Editor alongside the native preview.
    SplitNative,
    /// Editor alongside the WebView preview.
    SplitWeb,
}

impl Layout {
    pub const ALL: [Layout; 5] = [
        Layout::Source,
        Layout::Native,
        Layout::Web,
        Layout::SplitNative,
        Layout::SplitWeb,
    ];

    /// The string key for this layout's label.
    pub fn label_key(self) -> crate::i18n::Key {
        match self {
            Layout::Source => crate::i18n::Key::ModeSource,
            Layout::Native => crate::i18n::Key::ModeNative,
            Layout::Web => crate::i18n::Key::ModeWeb,
            Layout::SplitNative => crate::i18n::Key::ModeSplitNative,
            Layout::SplitWeb => crate::i18n::Key::ModeSplitWeb,
        }
    }

    /// Stable id, used for element ids and settings.
    ///
    /// Never translated: an id that changed with the language would break every
    /// test and keybinding referring to it.
    pub fn key(self) -> &'static str {
        match self {
            Layout::Source => "source",
            Layout::Native => "native",
            Layout::Web => "web",
            Layout::SplitNative => "split-native",
            Layout::SplitWeb => "split-web",
        }
    }

    pub fn from_key(key: &str) -> Option<Layout> {
        Self::ALL.into_iter().find(|l| l.key() == key)
    }

    /// Whether the editor is visible.
    pub fn shows_editor(self) -> bool {
        matches!(
            self,
            Layout::Source | Layout::SplitNative | Layout::SplitWeb
        )
    }

    /// Whether a preview is visible.
    pub fn shows_preview(self) -> bool {
        self != Layout::Source
    }

    /// Whether the editor and a preview are side by side.
    pub fn is_split(self) -> bool {
        matches!(self, Layout::SplitNative | Layout::SplitWeb)
    }

    /// Whether this layout drives the WebView.
    ///
    /// No second argument: the layout already names its renderer, which is the
    /// point of collapsing the two enums.
    pub fn uses_webview(self) -> bool {
        matches!(self, Layout::Web | Layout::SplitWeb)
    }

    /// The preview renderer this layout uses, if it shows one.
    pub fn preview(self) -> Option<PreviewKind> {
        match self {
            Layout::Source => None,
            Layout::Native | Layout::SplitNative => Some(PreviewKind::Native),
            Layout::Web | Layout::SplitWeb => Some(PreviewKind::Web),
        }
    }

    /// This layout with the editor shown, for a jump that needs a cursor.
    ///
    /// A preview-only layout has nowhere to put a caret, so revealing an offset
    /// in one has to open the editor. Keeping the user's chosen renderer is the
    /// part that matters: forcing a bare "Split" silently swapped a Web preview
    /// for a native one.
    pub fn with_editor(self) -> Layout {
        match self {
            Layout::Native => Layout::SplitNative,
            Layout::Web => Layout::SplitWeb,
            other => other,
        }
    }
}

/// Which renderer a preview pane uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Native,
    Web,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_visibility() {
        assert!(Layout::Source.shows_editor());
        assert!(!Layout::Source.shows_preview());
        assert!(!Layout::Native.shows_editor());
        assert!(Layout::Native.shows_preview());
        assert!(Layout::SplitNative.shows_editor() && Layout::SplitNative.shows_preview());
        assert!(Layout::SplitWeb.shows_editor() && Layout::SplitWeb.shows_preview());
    }

    #[test]
    fn every_layout_shows_at_least_one_pane() {
        // A layout showing neither the editor nor a preview would render an
        // empty document area.
        for layout in Layout::ALL {
            assert!(
                layout.shows_editor() || layout.shows_preview(),
                "{} shows nothing",
                layout.key()
            );
        }
    }

    #[test]
    fn every_layout_names_exactly_one_preview_renderer() {
        // The reason for collapsing the two enums: `Split` used to mean two
        // different layouts depending on a separate control, so "which renderer
        // is showing" could not be answered from the mode alone.
        for layout in Layout::ALL {
            match layout.preview() {
                None => assert!(!layout.shows_preview(), "{}", layout.key()),
                Some(_) => assert!(layout.shows_preview(), "{}", layout.key()),
            }
        }
        assert_eq!(Layout::Source.preview(), None);
        assert_eq!(Layout::Native.preview(), Some(PreviewKind::Native));
        assert_eq!(Layout::SplitWeb.preview(), Some(PreviewKind::Web));
    }

    #[test]
    fn the_webview_is_used_by_exactly_the_web_layouts() {
        let web: Vec<&str> = Layout::ALL
            .iter()
            .filter(|l| l.uses_webview())
            .map(|l| l.key())
            .collect();
        assert_eq!(web, vec!["web", "split-web"]);
        // And that agrees with the renderer each names, so the two cannot drift.
        for layout in Layout::ALL {
            assert_eq!(
                layout.uses_webview(),
                layout.preview() == Some(PreviewKind::Web),
                "{}",
                layout.key()
            );
        }
    }

    #[test]
    fn keys_round_trip_and_are_distinct() {
        for layout in Layout::ALL {
            assert_eq!(Layout::from_key(layout.key()), Some(layout));
        }
        assert_eq!(Layout::from_key("nonsense"), None);
        let keys: std::collections::HashSet<&str> = Layout::ALL.iter().map(|l| l.key()).collect();
        assert_eq!(keys.len(), Layout::ALL.len());
    }

    #[test]
    fn all_layouts_have_distinct_labels() {
        use crate::i18n::text;
        use crate::settings::Language;

        for language in Language::ALL {
            let labels: std::collections::HashSet<&str> = Layout::ALL
                .iter()
                .map(|l| text(l.label_key(), language))
                .collect();
            assert_eq!(
                labels.len(),
                Layout::ALL.len(),
                "duplicate labels in {}",
                language.label()
            );
        }
    }

    #[test]
    fn showing_the_editor_keeps_the_chosen_renderer() {
        // The bug this replaces: clicking an outline entry in Web mode forced
        // the document to Split *with the native preview*, silently discarding
        // the renderer the user had picked.
        assert_eq!(Layout::Web.with_editor(), Layout::SplitWeb);
        assert_eq!(Layout::Native.with_editor(), Layout::SplitNative);
        // Layouts that already show the editor are unchanged.
        for layout in Layout::ALL.iter().filter(|l| l.shows_editor()) {
            assert_eq!(layout.with_editor(), *layout);
        }
        // And the result always has an editor to put a cursor in.
        for layout in Layout::ALL {
            assert!(layout.with_editor().shows_editor(), "{}", layout.key());
        }
    }

    #[test]
    fn split_is_exactly_the_two_side_by_side_layouts() {
        let split: Vec<&str> = Layout::ALL
            .iter()
            .filter(|l| l.is_split())
            .map(|l| l.key())
            .collect();
        assert_eq!(split, vec!["split-native", "split-web"]);
        for layout in Layout::ALL {
            assert_eq!(
                layout.is_split(),
                layout.shows_editor() && layout.shows_preview(),
                "{}",
                layout.key()
            );
        }
    }
}
