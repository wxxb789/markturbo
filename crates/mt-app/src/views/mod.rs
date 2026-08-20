//! GPUI views.
//!
//! Views read the document engine; they never own document semantics. That
//! keeps the engine reusable headless and keeps each view small.

pub mod document;
pub mod explorer;
pub mod skills;
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
    cx.with_window(entity.entity_id(), |_, app| {
        app.update_entity(&entity, f)
    })
}

/// Which view of a document is showing.
///
/// Every document conceptually supports all four; a view that does not apply
/// (Web on a platform without a WebView) degrades with an explanation rather
/// than disappearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Native lightweight editor.
    Source,
    /// GPUI-native rendering: the fast path.
    Native,
    /// WebView rendering: the compatibility path.
    Web,
    /// Source alongside a preview.
    Split,
}

impl ViewMode {
    pub const ALL: [ViewMode; 4] = [
        ViewMode::Source,
        ViewMode::Native,
        ViewMode::Web,
        ViewMode::Split,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Source => "Source",
            ViewMode::Native => "Native",
            ViewMode::Web => "Web",
            ViewMode::Split => "Split",
        }
    }

    /// Whether the editor is visible in this mode.
    pub fn shows_editor(self) -> bool {
        matches!(self, ViewMode::Source | ViewMode::Split)
    }

    /// Whether a preview is visible in this mode.
    pub fn shows_preview(self) -> bool {
        !matches!(self, ViewMode::Source)
    }

    /// Whether this mode drives the WebView.
    pub fn uses_webview(self, split_preview: PreviewKind) -> bool {
        match self {
            ViewMode::Web => true,
            ViewMode::Split => split_preview == PreviewKind::Web,
            _ => false,
        }
    }
}

/// Which renderer the preview pane uses in Split mode.
///
/// Separating this from [`ViewMode`] is what leaves room for `Native | Web` and
/// `Original | Translation` layouts without reworking the mode enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Native,
    Web,
}

impl PreviewKind {
    pub fn label(self) -> &'static str {
        match self {
            PreviewKind::Native => "Native preview",
            PreviewKind::Web => "Web preview",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_visibility() {
        assert!(ViewMode::Source.shows_editor());
        assert!(!ViewMode::Source.shows_preview());
        assert!(!ViewMode::Native.shows_editor());
        assert!(ViewMode::Native.shows_preview());
        assert!(ViewMode::Split.shows_editor() && ViewMode::Split.shows_preview());
    }

    #[test]
    fn webview_is_used_only_where_expected() {
        assert!(ViewMode::Web.uses_webview(PreviewKind::Native));
        assert!(!ViewMode::Native.uses_webview(PreviewKind::Web));
        assert!(!ViewMode::Source.uses_webview(PreviewKind::Web));
        assert!(ViewMode::Split.uses_webview(PreviewKind::Web));
        assert!(!ViewMode::Split.uses_webview(PreviewKind::Native));
    }

    #[test]
    fn all_modes_have_distinct_labels() {
        let labels: std::collections::HashSet<_> =
            ViewMode::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(labels.len(), ViewMode::ALL.len());
    }

    #[test]
    fn every_mode_shows_at_least_one_pane() {
        // A mode that shows neither the editor nor a preview would render an
        // empty document area.
        for mode in ViewMode::ALL {
            assert!(
                mode.shows_editor() || mode.shows_preview(),
                "{} shows nothing",
                mode.label()
            );
        }
    }

    #[test]
    fn the_preview_choice_only_matters_in_split() {
        // Native and Source ignore it entirely; Web always uses the WebView.
        // This is what lets one predicate drive the pane, the HTML rebuild, and
        // the workspace's WebView sync without them disagreeing.
        for kind in [PreviewKind::Native, PreviewKind::Web] {
            assert!(!ViewMode::Source.uses_webview(kind));
            assert!(!ViewMode::Native.uses_webview(kind));
            assert!(ViewMode::Web.uses_webview(kind));
        }
        assert!(ViewMode::Split.uses_webview(PreviewKind::Web));
        assert!(!ViewMode::Split.uses_webview(PreviewKind::Native));
    }

    #[test]
    fn native_is_the_default_path_for_reading() {
        // The conceptual rule: native is the fast path. Only two of four modes
        // reach the WebView at all, and neither is a reading default.
        let web_modes = ViewMode::ALL
            .iter()
            .filter(|m| m.uses_webview(PreviewKind::Web))
            .count();
        assert_eq!(web_modes, 2, "only Web and Split-with-Web use the WebView");
    }
}
