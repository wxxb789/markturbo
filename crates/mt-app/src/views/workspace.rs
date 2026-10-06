//! The workspace: explorer + skills + outline on the left, tabbed documents on
//! the right.
//!
//! Owns the open-document set, the filesystem watcher, the Web preview surface,
//! and the commands (open folder, save, translate). Individual views stay
//! narrow; this is where they are wired together.
//!
//! Cohesive state lives in submodules where it has its own owner: `history`
//! holds navigation data and controls, `recovery` owns the app recovery flow,
//! `review` owns the Review → Revision workflow, and `web_surface` owns the OS
//! child window and re-entrancy rules. `welcome` owns first-use presentation
//! and recent-target availability. The view modules add methods to `Workspace`,
//! so existing wiring reads the same while those responsibilities stay together.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::base::{Button as BaseButton, GlobalState, Toggle as BaseToggle};
use gpui_kit::component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, StyledExt as _,
    TITLE_BAR_HEIGHT as COMPONENT_TITLE_BAR_HEIGHT, ThemeStyled as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    menu::ContextMenuExt as _,
    spinner::Spinner,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mt_core::agent_artifacts::package::FrozenSkillPackage;
use mt_core::translate::provider::PreparedTranslation;
use mt_core::translate::{Scope, TranslationRequest};

use crate::i18n;
use crate::metrics;
use crate::startup::{AcknowledgeStartupInput, InitialStartupState, StartupEvent};
use crate::views::document::{DocumentEvent, DocumentView, SaveAsMode, SaveAsOutcome, SaveMode};
use crate::views::explorer::{Explorer, ExplorerEvent};
use crate::views::harness::{HarnessEvent, HarnessView};
use crate::views::search::{Corpus, OpenSnapshot, SearchEvent, SearchView};
use crate::views::settings_page::{SettingsEvent, SettingsView};
use mt_core::document::io as fs;
use mt_core::document::lifecycle::{
    DestructiveAction, DestructiveRequest, DestructiveResolution, DirtyDecision, DocumentId,
    DocumentLifecycle,
};
use mt_core::model::{ConsentCapability, ConsentDecision};
use mt_core::recovery::{RecoveryKey, RevisionRecovery};
use mt_core::rendering::RendererRegistry;
use mt_core::workspace::search::SearchTarget;
use mt_core::workspace::tabs::{TabIdentity, Tabs};
use mt_core::workspace::watcher::{Change, Watcher};

mod history;
mod recovery;
mod review;
pub(crate) mod web_surface;
mod welcome;

#[cfg(test)]
use self::recovery::{
    DocumentRecoveryState, RecoveryAttempt, RecoveryContentIdentity, checkpoint_batch_status,
    current_checkpoint_write_completed_for_identity, prepare_recovery_records,
    startup_recovery_status,
};
use self::recovery::{RecoveryFlow, StartupRecovery};
use self::review::{
    ReviewFlow, RevisionSaveAsApproval, RevisionSaveOutcome, export_recovered_revision_answers,
};
#[cfg(test)]
use self::review::{
    WorkspaceReviewResult, WorkspaceRevisionContext, WorkspaceRevisionResult,
    build_revision_recovery, next_review_lens, revision_answer_is_incorporated,
    revision_apply_identity_matches,
};
use self::web_surface::WebSurface;
use self::welcome::WelcomeState;
#[cfg(test)]
use mt_core::agent_artifacts::package::{
    ReviewRequestBuildError, ReviewRequestBuildRequest, ReviewTarget, build_review_request,
};
use mt_core::workspace::History;

actions!(
    markturbo,
    [
        NewDocument,
        PasteIntoNew,
        OpenFile,
        OpenFolder,
        Save,
        SaveAs,
        CloseTab,
        OpenSettings,
        TranslateDocument,
        TranslateSelection,
        TranslateBlock,
        ReviewDocument,
        ReviewSelection,
        CancelReview,
        CopyPath,
        CopyRelativePath,
        ToggleLeftPanel,
        ToggleRightPanel,
        FocusSearch,
        NavigateBack,
        NavigateForward
    ]
);

/// The longest tab label before it is elided.
///
/// Long enough for `architecture.md` and most agent-artifact names, short
/// enough that six open documents still fit across a laptop window. A tab that
/// grows to its file name pushes every other tab off the bar, which is the
/// failure this bounds — the full path is a hover away.
const TAB_LABEL_MAX: usize = 22;
const TAB_CLOSE_ACCESSIBILITY_ID: &str = "markturbo-document-tab-close";
const REVIEW_RUN_ACCESSIBILITY_ID: &str = "markturbo-review-run";
const REVIEW_DIAGNOSTIC_ACCESSIBILITY_ID: &str = "markturbo-review-diagnostic";
const REVIEW_RESULT_ACCESSIBILITY_ID: &str = "markturbo-review-result";
const REVISION_RUN_ACCESSIBILITY_ID: &str = "markturbo-revision-run";
const REVISION_RETRY_ACCESSIBILITY_ID: &str = "markturbo-revision-retry";
const REVISION_DISMISS_ACCESSIBILITY_ID: &str = "markturbo-revision-dismiss";
const REVISION_ACCEPT_ALL_ACCESSIBILITY_ID: &str = "markturbo-revision-accept-all";
const REVISION_REJECT_ALL_ACCESSIBILITY_ID: &str = "markturbo-revision-reject-all";
const REVISION_APPLY_ACCESSIBILITY_ID: &str = "markturbo-revision-apply";
const REVISION_COPY_ACCESSIBILITY_ID: &str = "markturbo-revision-copy";
const REVISION_SAVE_ACCESSIBILITY_ID: &str = "markturbo-revision-save";
const REVISION_SAVE_AS_ACCESSIBILITY_ID: &str = "markturbo-revision-save-as";
const REVISION_STALE_ACCESSIBILITY_ID: &str = "markturbo-revision-stale";
const REVISION_RESULT_DISMISS_ACCESSIBILITY_ID: &str = "markturbo-revision-result-dismiss";
const REVISION_DISCARD_ANSWERS_ACCESSIBILITY_ID: &str = "markturbo-revision-discard-answers";
const REVISION_RECOVERED_ANSWERS_ACCESSIBILITY_ID: &str = "markturbo-revision-recovered-answers";
const REVISION_COPY_RECOVERED_ANSWERS_ACCESSIBILITY_ID: &str =
    "markturbo-revision-copy-recovered-answers";
const REVISION_DISCARD_RECOVERED_ANSWERS_ACCESSIBILITY_ID: &str =
    "markturbo-revision-discard-recovered-answers";

/// Shorten `name` to [`TAB_LABEL_MAX`], keeping the extension.
///
/// The extension is what distinguishes `notes.md` from `notes.mdx`, so eliding
/// from the end — the obvious implementation — removes exactly the part worth
/// keeping. This elides the stem instead.
fn elide_tab_label(name: &str) -> String {
    let count = name.chars().count();
    if count <= TAB_LABEL_MAX {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        // A leading dot is a hidden file, not an extension.
        Some(ix) if ix > 0 => (&name[..ix], &name[ix..]),
        _ => (name, ""),
    };
    let ext_len = ext.chars().count();
    // Keep at least a few characters of the stem, even beside a long extension.
    let keep = TAB_LABEL_MAX.saturating_sub(ext_len + 1).max(3);
    let head: String = stem.chars().take(keep).collect();
    format!("{head}…{ext}")
}

/// Which left-panel section is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Files,
    Search,
    Harness,
    Outline,
}

impl SidePanel {
    const ALL: [SidePanel; 4] = [
        SidePanel::Files,
        SidePanel::Search,
        SidePanel::Harness,
        SidePanel::Outline,
    ];

    /// The string key for this panel, resolved against the chosen language
    /// at render time rather than baked in here.
    fn label(self) -> crate::i18n::Key {
        match self {
            SidePanel::Files => crate::i18n::Key::PanelFiles,
            SidePanel::Search => crate::i18n::Key::PanelSearch,
            SidePanel::Harness => crate::i18n::Key::PanelHarness,
            SidePanel::Outline => crate::i18n::Key::PanelOutline,
        }
    }

    fn id(self) -> &'static str {
        match self {
            SidePanel::Files => "side-panel-files",
            SidePanel::Search => "side-panel-search",
            SidePanel::Harness => "side-panel-harness",
            SidePanel::Outline => "side-panel-outline",
        }
    }

    fn icon(self) -> IconName {
        match self {
            SidePanel::Files => IconName::Folder,
            SidePanel::Search => IconName::Search,
            SidePanel::Harness => IconName::Bot,
            SidePanel::Outline => IconName::BookOpen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailsContent {
    Empty,
    Document,
    Harness,
}

fn details_content(
    side_panel: SidePanel,
    settings_open: bool,
    document_open: bool,
    harness_selected: bool,
) -> DetailsContent {
    if settings_open {
        DetailsContent::Empty
    } else if side_panel == SidePanel::Harness && harness_selected {
        DetailsContent::Harness
    } else if document_open {
        DetailsContent::Document
    } else {
        DetailsContent::Empty
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WorkspacePanelWidths {
    left: Pixels,
    right: Pixels,
}

fn resolved_workspace_panel_widths(
    preferred_left: Pixels,
    preferred_right: Pixels,
    left_visible: bool,
    right_visible: bool,
    viewport: Pixels,
) -> WorkspacePanelWidths {
    let requested_left = if left_visible { preferred_left } else { px(0.) };
    let requested_right = if right_visible {
        preferred_right
    } else {
        px(0.)
    };

    let left_range = metrics::SIDE_PANEL.drag_range();
    let right_range = metrics::RIGHT_PANEL.drag_range();
    let mut left = requested_left.clamp(left_range.start, left_range.end);
    let mut right = requested_right.clamp(right_range.start, right_range.end);
    // A width chosen on a large display is still a preference after the window
    // shrinks, not permission to squeeze the document out of existence. Reuse
    // the side panel's established useful-width floor for the center column;
    // when both panels are visible, distribute the remaining budget in the
    // same proportion as the user's requested extra width above each minimum.
    let side_budget = (viewport - px(metrics::DOCUMENT_MIN)).max(px(0.));
    match (left_visible, right_visible) {
        (true, true) => {
            let minimum = left_range.start + right_range.start;
            let total = left + right;
            if total > side_budget {
                if side_budget < minimum {
                    left = if minimum > px(0.) {
                        side_budget * (left_range.start / minimum)
                    } else {
                        px(0.)
                    };
                    right = side_budget - left;
                    return WorkspacePanelWidths { left, right };
                }
                let left_extra = (left - left_range.start).max(px(0.));
                let right_extra = (right - right_range.start).max(px(0.));
                let total_extra = left_extra + right_extra;
                let extra_budget = side_budget - minimum;
                if total_extra > px(0.) {
                    left = left_range.start + extra_budget * (left_extra / total_extra);
                    right = side_budget - left;
                } else {
                    left = left_range.start;
                    right = right_range.start;
                }
            }
        }
        (true, false) => {
            left = left.min(side_budget);
            right = px(0.);
        }
        (false, true) => {
            let maximum = side_budget.min(right_range.end);
            left = px(0.);
            right = right.min(maximum);
        }
        (false, false) => {
            left = px(0.);
            right = px(0.);
        }
    }

    WorkspacePanelWidths { left, right }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceResizeEdge {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct WorkspaceResizeGrab {
    edge: WorkspaceResizeEdge,
    pointer_offset: Pixels,
}

#[derive(Debug, Clone, Copy)]
struct WorkspaceResizeGeometry {
    boundary: Pixels,
    width: Pixels,
    minimum: Pixels,
    maximum: Pixels,
}

struct WorkspaceResizePreview;

impl Render for WorkspaceResizePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn clamped_dragged_panel_width(
    edge: WorkspaceResizeEdge,
    requested: Pixels,
    opposite_width: Pixels,
    opposite_visible: bool,
    viewport: Pixels,
) -> Pixels {
    let (minimum, maximum) = panel_width_limits(edge, opposite_width, opposite_visible, viewport);
    requested.clamp(minimum, maximum)
}

fn panel_width_limits(
    edge: WorkspaceResizeEdge,
    opposite_width: Pixels,
    opposite_visible: bool,
    viewport: Pixels,
) -> (Pixels, Pixels) {
    let configured = match edge {
        WorkspaceResizeEdge::Left => metrics::SIDE_PANEL.drag_range(),
        WorkspaceResizeEdge::Right => metrics::RIGHT_PANEL.drag_range(),
    };
    let opposite_width = if opposite_visible {
        opposite_width
    } else {
        px(0.)
    };
    let available = (viewport - px(metrics::DOCUMENT_MIN) - opposite_width).max(px(0.));
    let maximum = configured.end.min(available);
    let minimum = configured.start.min(maximum);
    (minimum, maximum)
}

fn workspace_resize_geometry(
    edge: WorkspaceResizeEdge,
    widths: WorkspacePanelWidths,
    left_visible: bool,
    right_visible: bool,
    viewport: Pixels,
) -> WorkspaceResizeGeometry {
    let (boundary, width, opposite_width, opposite_visible) = match edge {
        WorkspaceResizeEdge::Left => (widths.left, widths.left, widths.right, right_visible),
        WorkspaceResizeEdge::Right => (
            viewport - widths.right,
            widths.right,
            widths.left,
            left_visible,
        ),
    };
    let (minimum, maximum) = panel_width_limits(edge, opposite_width, opposite_visible, viewport);
    WorkspaceResizeGeometry {
        boundary,
        width,
        minimum,
        maximum,
    }
}

fn workspace_region(id: &'static str, width: Option<Pixels>, content: AnyElement) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .h_full()
        .min_w_0()
        .min_h_0()
        .when_some(width, |this, width| this.w(width).flex_none())
        .when(width.is_none(), |this| this.flex_1())
        .child(content)
        .into_any_element()
}

fn native_window_controls_width(window: &Window) -> Pixels {
    if cfg!(target_os = "macos") || cfg!(target_family = "wasm") {
        return px(0.);
    }

    #[cfg(target_os = "linux")]
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return px(0.);
    }

    let supported = window.window_controls();
    let controls = 1 + usize::from(supported.minimize) + usize::from(supported.maximize);
    COMPONENT_TITLE_BAR_HEIGHT * controls as f32
}

type ChromeClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Icon-only workspace command with Base-owned focus, keyboard, and accessible
/// naming, styled from the same theme tokens as gpui-component's ghost button.
///
/// The pinned styled `Button` derives its accessible name only from its visible
/// label. This narrow composition keeps the requested icon-only presentation
/// without leaving WebView mode with anonymous controls when popup tooltips are
/// deliberately suppressed.
#[derive(IntoElement)]
pub(super) struct ChromeIconButton {
    id: &'static str,
    icon: IconName,
    label: SharedString,
    pressed: Option<bool>,
    disabled: bool,
    loading: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ChromeClickHandler>,
}

impl ChromeIconButton {
    pub(super) fn new(id: &'static str, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            id,
            icon,
            label: label.into(),
            pressed: None,
            disabled: false,
            loading: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub(super) fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = Some(pressed);
        self
    }

    pub(super) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub(super) fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub(super) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for ChromeIconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = window
            .use_keyed_state(self.id, cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let is_focused = focus_handle.is_focused(window);
        let normal_foreground = cx.theme().secondary_foreground;
        let hover_background = cx.theme().tokens.secondary_hover.background;
        let active_background = cx.theme().tokens.secondary_active.background;
        let disabled_foreground = cx.theme().muted_foreground.opacity(0.5);
        let icon = if self.loading {
            Spinner::new().small().into_any_element()
        } else {
            Icon::new(self.icon).small().into_any_element()
        };
        let on_click = self.on_click;
        let loading = self.loading;
        let disabled = self.disabled;
        let inert = disabled || loading;
        let inactive_foreground = if loading {
            normal_foreground
        } else {
            disabled_foreground
        };
        let button = match self.pressed {
            Some(pressed) => BaseToggle::new(self.id)
                .pressed(pressed)
                .disabled(inert)
                .accessibility_label(self.label)
                .track_focus(&focus_handle)
                .styles(|styles| {
                    styles
                        .pressed(|style| style.bg(active_background).text_color(normal_foreground))
                        .disabled(|style| {
                            style
                                .bg(cx.theme().transparent)
                                .text_color(inactive_foreground)
                        })
                })
                .flex()
                .flex_shrink_0()
                .size(metrics::target())
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .bg(cx.theme().transparent)
                .text_color(normal_foreground)
                .when(!inert && !pressed, |this| {
                    this.hover(|style| style.bg(hover_background))
                        .active(|style| style.bg(active_background))
                })
                .when(!inert, |this| {
                    this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        GlobalState::suppress_text_selection(cx);
                    })
                })
                .when_some(on_click.filter(|_| !inert), |this, on_click| {
                    this.on_change(move |_, event, window, cx| on_click(event, window, cx))
                })
                .child(icon)
                .when(is_focused && !inert, |this| {
                    this.focus_ring_style(window, cx)
                })
                .into_any_element(),
            None => BaseButton::new(self.id)
                .disabled(inert)
                .accessibility_label(self.label)
                .track_focus(&focus_handle)
                .styles(|styles| {
                    styles.disabled(|style| {
                        style
                            .bg(cx.theme().transparent)
                            .text_color(inactive_foreground)
                    })
                })
                .flex()
                .flex_shrink_0()
                .size(metrics::target())
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .bg(cx.theme().transparent)
                .text_color(normal_foreground)
                .when(!inert, |this| {
                    this.hover(|style| style.bg(hover_background))
                        .active(|style| style.bg(active_background))
                })
                .when(loading, |this| this.opacity(0.8))
                .when(!inert, |this| {
                    this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        GlobalState::suppress_text_selection(cx);
                    })
                })
                .when_some(on_click.filter(|_| !inert), |this, on_click| {
                    this.on_click(move |event, window, cx| on_click(event, window, cx))
                })
                .child(icon)
                .when(is_focused && !inert, |this| {
                    this.focus_ring_style(window, cx)
                })
                .into_any_element(),
        };

        if let Some(tooltip) = self.tooltip {
            div()
                .id(SharedString::from(format!("chrome-tooltip-{}", self.id)))
                .flex()
                .flex_shrink_0()
                .child(button)
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .into_any_element()
        } else {
            button
        }
    }
}

/// Full-width sidebar navigation with application-owned content alignment.
///
/// The styled component centers its private label row after caller styles are
/// applied, so `w_full().justify_start()` still rendered these four entries in
/// the middle of the panel. Base keeps the semantic button behavior while the
/// visible row shares the file tree's 8px content spine.
#[derive(IntoElement)]
struct SidebarNavigationButton {
    id: &'static str,
    icon: IconName,
    label: SharedString,
    selected: bool,
    on_click: Option<ChromeClickHandler>,
}

impl SidebarNavigationButton {
    fn new(id: &'static str, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            id,
            icon,
            label: label.into(),
            selected: false,
            on_click: None,
        }
    }

    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SidebarNavigationButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = window
            .use_keyed_state(self.id, cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let is_focused = focus_handle.is_focused(window);
        let label = self.label.clone();
        let selected = self.selected;
        let on_click = self.on_click;
        let foreground = cx.theme().sidebar_foreground;
        let selected_background = cx.theme().sidebar_accent;
        let hover_background = cx.theme().tokens.secondary_hover.background;
        let active_background = cx.theme().tokens.secondary_active.background;

        BaseToggle::new(self.id)
            .pressed(selected)
            .accessibility_label(self.label)
            .track_focus(&focus_handle)
            .styles(|styles| {
                styles.pressed(|style| {
                    style
                        .bg(selected_background)
                        .text_color(cx.theme().sidebar_accent_foreground)
                })
            })
            .flex()
            .w_full()
            .h(metrics::row())
            .flex_shrink_0()
            .items_center()
            .justify_start()
            .px(metrics::row_pad())
            .rounded(cx.theme().radius)
            .bg(cx.theme().transparent)
            .text_color(foreground)
            .when(!selected, |this| {
                this.hover(|style| style.bg(hover_background))
                    .active(|style| style.bg(active_background))
            })
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                GlobalState::suppress_text_selection(cx);
            })
            .when_some(on_click, |this, on_click| {
                this.on_change(move |_, event, window, cx| on_click(event, window, cx))
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_start()
                    .gap(metrics::gap())
                    .child(Icon::new(self.icon).small())
                    .child(div().min_w_0().truncate().text_sm().child(label)),
            )
            .when(is_focused, |this| this.focus_ring_style(window, cx))
    }
}

/// How often to drain the filesystem watcher.
///
/// The watcher itself is already debounced; this only governs how quickly a
/// detected change reaches the UI.
const WATCH_POLL: Duration = Duration::from_millis(500);

/// Whether a workspace path can change Harness discovery results.
///
/// The file tree reacts to every create/remove, but the Harness panel only
/// depends on conventional skill and instruction roots. Treating an ordinary
/// editor's temp-file rename as a Harness change restarted discovery and its
/// 250ms loading state on every save, which made both sections blink.
fn path_affects_harness(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative_text = relative
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let is_directory = path.is_dir() || relative.extension().is_none();

    for root in mt_core::agent_artifacts::skill::discovery_roots() {
        let root = root.to_ascii_lowercase();
        if root.starts_with(&format!("{relative_text}/")) || relative_text == root {
            return true;
        }
        let Some(within) = relative_text.strip_prefix(&format!("{root}/")) else {
            continue;
        };
        let components: Vec<&str> = within.split('/').collect();
        let name = components.last().copied().unwrap_or_default();
        if (components.len() <= 4 && name.eq_ignore_ascii_case("skill.md"))
            || (components.len() <= 3 && is_directory)
            || (components.len() <= 4 && matches!(name, "scripts" | "references" | "assets"))
        {
            return true;
        }
    }

    let is_instruction =
        mt_core::agent_artifacts::instruction::is_instruction(Path::new(&relative_text));
    let affects_instruction_root = |within: &str| {
        let components: Vec<&str> = within.split('/').collect();
        let first = components.first().copied().unwrap_or_default();
        let nested = matches!(first, "rules" | "instructions" | "memories");

        (components.len() == 1 && (nested || is_instruction))
            || (nested && components.len() <= 3 && is_instruction)
            || (nested && components.len() == 2 && is_directory)
    };
    for root in mt_core::agent_artifacts::instruction::project_roots()
        .iter()
        .filter(|root| !root.as_os_str().is_empty())
    {
        let root = root
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if root.starts_with(&format!("{relative_text}/")) || relative_text == root {
            return true;
        }
        let Some(within) = relative_text.strip_prefix(&format!("{root}/")) else {
            continue;
        };
        if affects_instruction_root(within) {
            return true;
        }
    }

    affects_instruction_root(&relative_text)
}

/// How long a status message stays on the bar.
///
/// Long enough to read a save confirmation without looking for it, short enough
/// that the bar is not still claiming "Saved" when the user comes back.
const STATUS_LINGER: Duration = Duration::from_secs(6);

fn document_details_status_key(is_externally_changed: bool, is_dirty: bool) -> i18n::Key {
    if is_externally_changed {
        i18n::Key::ChangedOnDisk
    } else if is_dirty {
        i18n::Key::UnsavedChanges
    } else {
        i18n::Key::Saved
    }
}

pub struct Workspace {
    focus_handle: FocusHandle,
    /// Retained scroll and entry-time availability for the Welcome surface.
    welcome: WelcomeState,
    /// The deliberate first-run surface, available only for a no-argument start.
    show_welcome: bool,
    root: Option<PathBuf>,
    explorer: Option<Entity<Explorer>>,
    harness: Option<Entity<HarnessView>>,
    /// Global skill roots outlive the folder-specific Harness view.
    skill_cache: Arc<Mutex<mt_core::agent_artifacts::skill::DiscoveryCache>>,
    search: Entity<SearchView>,
    /// The settings page, built once rather than per open.
    ///
    /// Eager like [`Self::search`] and for the same reason: the subscription
    /// that carries its events has to be set up somewhere, and a lazily-created
    /// entity would mean re-subscribing on every open — the pattern that leaks
    /// a subscription per click. It is stateless, so an unopened one costs a
    /// focus handle.
    settings: Entity<SettingsView>,
    side_panel: SidePanel,
    /// The open tabs, the active one, the preview slot, and the menu target.
    ///
    /// One field rather than five: the index arithmetic between them is where
    /// closing a tab used to switch documents, leak subscriptions, and strand
    /// the preview slot. [`Tabs`] owns those rules and tests them without a
    /// window.
    tabs: Tabs<DocumentTab>,
    /// Back/forward across visited positions.
    history: History,
    registry: Arc<RendererRegistry>,
    watcher: Option<Watcher>,
    /// One owner for startup arbitration, checkpoint timing, and retirement durability.
    recovery_flow: RecoveryFlow,
    status: Option<String>,
    /// Bumped by every [`Workspace::set_status`], so a timer can tell whether
    /// the message it was started for is still the one on screen.
    status_generation: u64,
    /// The timer that clears the current status message.
    ///
    /// One slot rather than a detached task per message: replacing it cancels
    /// the previous timer, which is the other half of the generation check.
    _status_timer: Option<Task<()>>,
    /// The window's single WebView and what it is showing.
    ///
    /// One field rather than three, and no `#[cfg]` here: the platform split
    /// is inside [`WebSurface`], which is empty on Linux. That is what keeps
    /// the dozen `web_dirty` call sites free of one.
    web: WebSurface,
    /// True while the settings page is showing.
    settings_open: bool,
    /// User-owned panel widths. Window resize only clamps the rendered result;
    /// it never rewrites these preferences, so maximize/restore is reversible.
    preferred_left_panel_width: Pixels,
    preferred_right_panel_width: Pixels,
    /// Actual width assigned by `Root`, which excludes Linux CSD shadow insets.
    /// `None` is used only for the first frame before prepaint measures it.
    layout_width: Option<Pixels>,
    /// Pointer-to-divider offset captured when a resize gesture begins.
    panel_resize_grab: Option<WorkspaceResizeGrab>,
    /// True while the file/harness/outline panel is showing on the left.
    left_panel_open: bool,
    /// True while the details panel is showing on the right.
    ///
    /// Not derived from whether anything is selected: a panel that appeared and
    /// vanished as the selection changed would resize the document under the
    /// user's cursor.
    right_panel_open: bool,
    /// True while a translation request is in flight.
    ///
    /// The button reads it through `loading`, which also makes it inert — a
    /// second request would overwrite the editor twice with two different
    /// answers to the same text.
    translating: bool,
    /// The Review → Approved Revision workflow has one app-side state owner.
    /// Its typed answer observations cross into the RecoveryFlow without
    /// duplicating answer state.
    review_flow: ReviewFlow,
    /// Present while Save / Discard / Cancel is resolving a destructive action.
    pending_destructive: Option<DestructiveRequest>,
    /// Invalidates path-picker and Replace callbacks from superseded requests.
    save_as_request_generation: u64,
    /// The active ticket and its origin-specific write authorization.
    pending_save_as: Option<PendingSaveAsRequest>,
    /// True after close is authorized and while the focused platform input
    /// handler drains across the final rendered frame.
    window_close_pending: bool,
    /// True only after input drain, authorizing the reposted native close.
    window_close_ready: bool,
    /// A fully resolved action waiting only for startup recovery to expose the
    /// guarded store needed to publish its durable retirement marker.
    pending_startup_destructive: Option<PendingStartupDestructive>,
    /// Recovery records handled by Save or Discard are retired only if the
    /// destructive walk reaches a safe lifecycle boundary. A later Cancel
    /// leaves every still-dirty buffer and its last checkpoint intact.
    pending_destructive_recovery: Vec<(RecoveryKey, Option<DocumentId>)>,
    _tasks: Vec<Task<()>>,
    #[cfg(test)]
    _test_recovery_root: Option<tempfile::TempDir>,
    /// Subscriptions that live as long as the workspace does.
    ///
    /// Per-document subscriptions are *not* here — they ride in
    /// [`DocumentTab`], so closing a tab drops them with it.
    _subscriptions: Vec<Subscription>,
    /// Subscriptions to the current folder's explorer and skills views,
    /// replaced wholesale when the folder changes.
    _panel_subscriptions: Vec<Subscription>,
}

/// What an open tab carries besides its path.
///
/// The subscriptions live here rather than in a workspace-wide `Vec` because
/// that `Vec` was only ever appended to: closing a tab removed the document and
/// left two subscriptions to it alive for the rest of the session.
struct DocumentTab {
    view: Entity<DocumentView>,
    _subscriptions: [Subscription; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveAsRequestOrigin {
    Normal,
    Destructive,
    Revision,
}

/// The sole Save As request allowed to consume an asynchronous picker answer.
#[derive(Clone, Copy)]
struct PendingSaveAsRequest {
    ticket: u64,
    document_id: DocumentId,
    origin: SaveAsRequestOrigin,
    revision_approval: Option<RevisionSaveAsApproval>,
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct PendingStartupDestructive {
    request: DestructiveRequest,
    keys: Vec<(RecoveryKey, Option<DocumentId>)>,
}

impl Workspace {
    /// Every open document, cloned.
    ///
    /// Cloned because every caller is about to `update` each one through the
    /// same `&mut Context` that borrows `self`.
    fn document_views(&self) -> Vec<Entity<DocumentView>> {
        self.tabs.iter().map(|t| t.payload.view.clone()).collect()
    }

    /// The document in the tab at `ix`.
    fn document_at(&self, ix: usize) -> Option<&Entity<DocumentView>> {
        self.tabs.get(ix).map(|t| &t.payload.view)
    }

    fn has_dirty_skill_supporting_document(&self, package: &FrozenSkillPackage, cx: &App) -> bool {
        self.document_views().into_iter().any(|document| {
            let document = document.read(cx);
            document.is_dirty()
                && document
                    .source_path()
                    .is_some_and(|path| package.contains_supporting_path(path))
        })
    }
}

impl Workspace {
    /// Create the workspace, opening `initial` if given.
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_startup_recovery(initial, recovery::startup_recovery, window, cx)
    }

    fn new_with_startup_recovery<F>(
        initial: Option<PathBuf>,
        load_startup_recovery: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        F: FnOnce() -> StartupRecovery + Send + 'static,
    {
        // Poll the watcher on a timer rather than blocking a thread on it: the
        // receiver is non-blocking and a UI tick is the natural cadence.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_POLL).await;
                if this.upgrade().is_none() {
                    break;
                }
                // A skipped tick is harmless: the watcher queue is drained on
                // the next one. A panic here would take the window with it.
                crate::views::try_update(&this, cx, |this, cx| this.drain_watcher(cx));
            }
        });
        let viewport = window.viewport_size().width;

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            welcome: WelcomeState::default(),
            show_welcome: welcome::should_show_welcome(
                initial.as_deref(),
                crate::settings::AppSettings::global(cx).show_welcome_on_startup,
            ),
            root: None,
            explorer: None,
            harness: None,
            skill_cache: Arc::new(Mutex::new(
                mt_core::agent_artifacts::skill::DiscoveryCache::default(),
            )),
            search: cx.new(|cx| SearchView::new(window, cx)),
            settings: cx.new(|cx| SettingsView::new(window, cx)),
            side_panel: SidePanel::Files,
            tabs: Tabs::default(),
            history: History::default(),
            registry: Arc::new(RendererRegistry::with_defaults()),
            watcher: None,
            recovery_flow: RecoveryFlow::new(),
            status: None,
            status_generation: 0,
            _status_timer: None,
            settings_open: false,
            preferred_left_panel_width: metrics::SIDE_PANEL.resolve(viewport),
            preferred_right_panel_width: metrics::RIGHT_PANEL.resolve(viewport),
            layout_width: None,
            panel_resize_grab: None,
            left_panel_open: true,
            right_panel_open: true,
            translating: false,
            review_flow: ReviewFlow::default(),
            pending_destructive: None,
            save_as_request_generation: 0,
            pending_save_as: None,
            window_close_pending: false,
            window_close_ready: false,
            pending_startup_destructive: None,
            pending_destructive_recovery: Vec::new(),
            web: WebSurface::default(),
            _tasks: vec![poll],
            #[cfg(test)]
            _test_recovery_root: None,
            _subscriptions: Vec::new(),
            _panel_subscriptions: Vec::new(),
        };

        if this.show_welcome {
            this.refresh_welcome_availability(cx);
        }

        // The saved preference, applied before the first frame so the window
        // never flashes the wrong theme.
        crate::settings::apply_theme(
            crate::settings::AppSettings::global(cx).theme,
            Some(window),
            cx,
        );

        // The search view cannot gather its own corpus — the open tabs' text is
        // in their editors and the harness paths in the harness view — so it
        // asks, and this answers.
        let search = this.search.clone();
        this._subscriptions.push(cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Ready => {
                    let corpus = this.search_corpus(cx);
                    this.search.update(cx, |search, cx| search.run(corpus, cx));
                }
                SearchEvent::Reveal {
                    path,
                    target,
                    offset,
                } => match target {
                    SearchTarget::File => this.reveal_in(path.clone(), *offset, window, cx),
                    SearchTarget::OpenDocument(id) => {
                        this.reveal_open_document(*id, path, *offset, window, cx)
                    }
                },
            },
        ));
        // The page writes the setting; what it cannot do is repaint the rest of
        // the app. Each event names what changed, and the response is chosen
        // here — the WebView caches HTML with the palette baked in, which is not
        // something a settings page should have to know.
        let settings = this.settings.clone();
        this._subscriptions.push(cx.subscribe_in(
            &settings,
            window,
            |this: &mut Self, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::ThemeChanged => this.reapply_theme(cx),
                SettingsEvent::LanguageChanged => this.relabel(cx),
                SettingsEvent::SkillScopeChanged => this.rescan_harness(cx),
                SettingsEvent::TestModelCredential => this.test_model_credential(window, cx),
            },
        ));
        // And the backstop, for the writers that are not the settings page.
        //
        // The status bar's watching toggle and the Harness panel's group-by
        // button both call `AppSettings::update` directly, and neither emits a
        // `SettingsEvent` — so before this, a setting changed from anywhere but
        // the settings page repainted only whichever view happened to own the
        // control. `global_mut` already pushes `NotifyGlobalObservers`, so
        // subscribing is the whole of what was missing.
        //
        // A plain redraw, not `relabel`: this fires for *every* settings write,
        // including the ones the subscription above is about to handle
        // specifically, and doing the expensive work twice would make each
        // dropdown change rebuild every open document's HTML.
        this._subscriptions.push(
            cx.observe_global::<crate::settings::AppSettings>(|this, cx| {
                this.review_flow
                    .observe_model_configuration(crate::settings::AppSettings::global(cx));
                cx.notify();
            }),
        );

        // The OS close button is only a request. Returning false keeps the
        // window alive while dirty documents walk the same decision boundary
        // as Ctrl/Cmd-W and the tab control.
        let workspace = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.request_window_close(window, cx)
                })
                .unwrap_or(true)
        });
        // Following the system means following it *while running*, not only at
        // startup — someone whose OS flips at sunset expects the app to flip
        // with it. An explicit preference ignores the event.
        let handle = cx.entity().downgrade();
        this._subscriptions
            .push(window.observe_window_appearance(move |window, cx| {
                if crate::settings::AppSettings::global(cx).theme
                    != mt_core::settings::ThemePreference::System
                {
                    return;
                }
                crate::settings::apply_theme(
                    mt_core::settings::ThemePreference::System,
                    Some(window),
                    cx,
                );
                // The Web preview caches HTML with the scheme baked in, so
                // recoloring GPUI alone would leave it on the old theme.
                if let Some(this) = handle.upgrade() {
                    this.update(cx, |this, cx| {
                        for doc in this.document_views() {
                            doc.update(cx, |doc, cx| doc.theme_changed(cx));
                        }
                        this.web_dirty(cx);
                    });
                }
            }));

        // Explicit targets are deliberately different from a no-argument
        // launch. `markturbo .` remains the terminal form for opening cwd.
        if let Some(path) = initial {
            this.open_target(path, false, window, cx);
        } else if !this.show_welcome {
            this.new_memory(String::new(), window, cx);
        }
        this.defer_startup_recovery(load_startup_recovery, window, cx);
        crate::startup::record(StartupEvent::InitialStateReady(if this.show_welcome {
            InitialStartupState::Welcome
        } else {
            InitialStartupState::Workspace
        }));
        this
    }

    /// Open `path` as the workspace root.
    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !path.is_dir() {
            self.set_status(format!("Not a directory: {}", path.display()), cx);
            return;
        }

        let explorer = cx.new(|cx| Explorer::new(path.clone(), window, cx));
        let skill_cache = self.skill_cache.clone();
        let harness = cx.new(|cx| HarnessView::new(path.clone(), skill_cache, window, cx));

        // Kept apart from `_subscriptions`: this set is replaced wholesale on
        // every folder change, and folding it into the general one would take
        // the window's appearance observer and every open document's observer
        // down with it.
        self._panel_subscriptions = vec![
            cx.subscribe_in(
                &explorer,
                window,
                |this: &mut Self, _, event: &ExplorerEvent, window, cx| {
                    let ExplorerEvent::OpenFile { path, preview } = event;
                    this.open_file_as(path.clone(), *preview, window, cx);
                },
            ),
            cx.subscribe_in(
                &harness,
                window,
                |this: &mut Self, _, event: &HarnessEvent, window, cx| match event {
                    HarnessEvent::OpenFile { path, preview } => {
                        this.open_file_as(path.clone(), *preview, window, cx);
                    }
                    HarnessEvent::ContextChanged { input } => {
                        this.resolve_effective_context(input.clone(), cx);
                    }
                },
            ),
            cx.observe(&harness, |this, _, cx| {
                this.web_dirty(cx);
                cx.notify();
            }),
        ];

        self.watcher = match Watcher::new(&path) {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                log::warn!("filesystem watching unavailable: {err}");
                None
            }
        };

        self.explorer = Some(explorer);
        self.harness = Some(harness);
        self.root = Some(path);
        self.show_welcome = false;
        let context_input = self
            .harness
            .as_ref()
            .and_then(|harness| harness.read(cx).context_input().cloned());
        self.resolve_effective_context(context_input, cx);
        self.sync_document_watches(cx);
        // Any results on screen came from the folder that was open a moment
        // ago. Leaving them would present another project's matches as this
        // one's, which is worse than an empty list.
        let search = self.search.clone();
        search.update(cx, |search, cx| search.rerun(cx));
        cx.notify();
    }

    fn sync_document_watches(&mut self, cx: &App) {
        let mut directories = HashSet::new();
        for document in self.document_views() {
            let document = document.read(cx);
            let Some(path) = document.source_path() else {
                continue;
            };
            if let Some(parent) = path.parent() {
                directories.insert(parent.to_path_buf());
            }
            if let Ok(resolved) = std::fs::canonicalize(path)
                && let Some(parent) = resolved.parent()
            {
                directories.insert(parent.to_path_buf());
            }
        }
        let mut context_paths = Vec::new();
        if let Some(harness) = &self.harness {
            let harness = harness.read(cx);
            if let Some(context) = harness.resolved_context() {
                context_paths = context.watch_paths();
                directories.extend(context.watch_directories());
            }
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
        watcher.sync_context_paths(context_paths);
        if let Err(err) = watcher.sync_document_directories(directories) {
            log::warn!("filesystem document watching could not be synchronized: {err}");
        }
    }

    /// Open a file in a tab, focusing an existing tab if it is already open.
    ///
    /// A pinned open: the tab stays until the user closes it.
    pub fn open_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open_file_as(path, false, window, cx)
    }

    /// Open a file, optionally as a preview.
    ///
    /// A preview reuses one slot: opening another preview replaces it rather
    /// than adding a tab, which is what keeps clicking through a tree from
    /// leaving a bar full of documents nobody asked to keep.
    pub fn open_file_as(
        &mut self,
        path: PathBuf,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Opening the file that is already the preview, by double click, is how
        // it gets promoted — the tab is already right, only its status changes.
        if preview {
            if self.tabs.is_preview(&path) {
                self.focus_path(&path, cx);
                return true;
            }
        } else if self.tabs.is_preview(&path) {
            self.tabs.set_preview(None);
            self.focus_path(&path, cx);
            return true;
        }

        let opened = self.open_file_inner(path.clone(), window, cx);
        if opened {
            self.retire_outgoing_preview(preview, cx);
            self.tabs.set_preview(preview.then_some(path));
        }
        cx.notify();
        opened
    }

    /// Replace an outgoing preview without discarding unsaved work.
    fn retire_outgoing_preview(&mut self, preview: bool, cx: &mut Context<Self>) {
        if !preview {
            return;
        }
        if let Some(current) = self.tabs.take_preview()
            && let Some(ix) = self.tabs.index_of(&current)
        {
            let preserve = self.tabs.get(ix).is_some_and(|tab| {
                let document = tab.payload.view.read(cx);
                let document_id = document.id();
                let key = self
                    .startup_recovery_key(document_id)
                    .cloned()
                    .unwrap_or_else(|| document.recovery_key());
                document.is_dirty()
                    || self.is_undurable_recovery_retirement(&key)
                    || self.revision_has_authored_answers_for_document(document_id, cx)
            });
            if !preserve {
                self.close_tab_unchecked(ix, cx);
            }
        }
    }

    /// Focus the tab showing `path`, if it is open.
    fn focus_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.tabs.focus_path(path) {
            self.record_visit(path.to_path_buf(), 0);
            self.web_dirty(cx);
            cx.notify();
        }
    }

    fn open_file_inner(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tabs.focus_path(&path) {
            // Opening a document while settings are showing has to show the
            // document, or the click in the explorer looks like it did nothing.
            self.settings_open = false;
            self.record_visit(path, 0);
            self.web_dirty(cx);
            cx.notify();
            return true;
        }

        let file = match fs::load(&path) {
            Ok(file) => file,
            Err(err) => {
                self.set_status(format!("Cannot open {}: {err}", path.display()), cx);
                return false;
            }
        };

        self.settings_open = false;
        let registry = self.registry.clone();
        let view = cx.new(|cx| DocumentView::new(file, registry, window, cx));
        self.insert_document(path.clone(), view, window, cx);
        true
    }

    /// Show the exact verified bytes of a frozen supporting file as a preview.
    /// A path-matching tab is reused only when its clean source still matches
    /// the verified load; an old clean buffer is not safe to present as an
    /// anchor into this package snapshot.
    fn open_frozen_supporting_file(
        &mut self,
        package: &FrozenSkillPackage,
        path: PathBuf,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = package.load_frozen_supporting_file(&path) else {
            self.mark_review_skill_package_stale(cx);
            return;
        };
        let loaded_path = file.path.clone();
        let existing = self.tabs.iter().enumerate().find_map(|(ix, tab)| {
            let path = tab.path()?;
            fs::paths_match(path, &loaded_path).then(|| (ix, path.to_path_buf()))
        });

        let shown_path = if let Some((ix, existing_path)) = existing {
            let Some(document) = self.document_at(ix).cloned() else {
                return;
            };
            let (is_dirty, matches_frozen_file) = {
                let document = document.read(cx);
                (document.is_dirty(), document.matches_loaded_file(&file, cx))
            };
            if is_dirty {
                self.mark_review_skill_package_stale(cx);
                return;
            }
            if !matches_frozen_file {
                self.set_status(i18n::t(i18n::Key::ReviewStale, cx).into(), cx);
                return;
            }

            // This exact identity was found in the tab set and its contents
            // were checked above, so the ordinary open path only focuses it;
            // it cannot fall through to a pathname read.
            if !self.open_file_as(existing_path.clone(), true, window, cx) {
                return;
            }
            existing_path
        } else {
            self.retire_outgoing_preview(true, cx);
            self.settings_open = false;
            let registry = self.registry.clone();
            let view = cx.new(|cx| DocumentView::new(file, registry, window, cx));
            self.insert_document(loaded_path.clone(), view, window, cx);
            self.tabs.set_preview(Some(loaded_path.clone()));
            cx.notify();
            loaded_path
        };

        let Some(document) = self.active_document().cloned() else {
            return;
        };
        if document.read(cx).source_path() != Some(shown_path.as_path()) {
            return;
        }
        self.record_visit(shown_path, offset);
        document.update(cx, |document, cx| {
            document.reveal_offset(offset, window, cx)
        });
    }

    /// Create a Markdown buffer before it has a filesystem path.
    pub fn new_memory(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.show_welcome = false;
        let registry = self.registry.clone();
        let view = cx.new(|cx| DocumentView::new_memory(text, registry, window, cx));
        self.insert_memory_document(view, true, window, cx);
    }

    /// Open either supported file or workspace target. User-driven folder
    /// changes retain the existing dirty-document interlock.
    fn open_target(
        &mut self,
        path: PathBuf,
        replace_workspace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if path.is_dir() {
            if replace_workspace {
                self.request_workspace_replace(path, window, cx);
            } else {
                self.open_folder(path.clone(), window, cx);
                self.record_recent_workspace(path, cx);
            }
            return true;
        }
        self.open_file_target(path, window, cx)
    }

    /// Files opened outside a workspace acquire a root for normal explorer,
    /// watcher, and relative-path behavior, but that parent is not itself a
    /// recently opened workspace.
    fn open_file_target(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !path.is_file() || !mt_core::workspace::is_openable(&path) {
            self.set_status(format!("Cannot open {}", path.display()), cx);
            return false;
        }
        let opened = self.open_file(path.clone(), window, cx);
        if opened {
            if self.root.is_none()
                && let Some(parent) = path.parent()
            {
                self.open_folder(parent.to_path_buf(), window, cx);
            }
            self.record_recent_file(path, cx);
        }
        opened
    }

    fn insert_document(
        &mut self,
        path: PathBuf,
        view: Entity<DocumentView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_document_with_recovery(TabIdentity::File(path), view, true, window, cx);
    }

    fn insert_memory_document(
        &mut self,
        view: Entity<DocumentView>,
        arm_dirty_recovery: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = view.read(cx).id();
        self.insert_document_with_recovery(
            TabIdentity::Memory(id),
            view,
            arm_dirty_recovery,
            window,
            cx,
        );
    }

    fn insert_document_with_recovery(
        &mut self,
        identity: TabIdentity,
        view: Entity<DocumentView>,
        arm_dirty_recovery: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_welcome = false;
        if arm_dirty_recovery && matches!(&identity, TabIdentity::File(_)) {
            self.isolate_reopened_file_recovery(&view, cx);
        }
        self.remember_startup_recovery_key(&view, cx);
        // Both subscriptions ride with the tab, so closing it drops them.
        let subscriptions = [
            cx.subscribe_in(
                &view,
                window,
                |this: &mut Self, document, event: &DocumentEvent, window, cx| match event {
                    DocumentEvent::Status(message) => this.set_status(message.clone(), cx),
                    DocumentEvent::Conflict => this.set_status(
                        "This file changed on disk. Reload or overwrite from the banner.".into(),
                        cx,
                    ),
                    DocumentEvent::SaveAsRequested => {
                        let id = document.read(cx).id();
                        this.prompt_save_as(id, window, cx);
                    }
                    DocumentEvent::Edited => {
                        this.mark_review_stale_for_document(document, cx);
                        this.refresh_revision_applied_state(document, cx);
                        this.note_document_edited(document, cx);
                    }
                    DocumentEvent::DirtyChanged => {
                        this.mark_review_stale_for_document(document, cx);
                        let id = document.read(cx).id();
                        if !document.read(cx).is_dirty()
                            && !this.revision_has_authored_answers_for_document(id, cx)
                        {
                            let current_key = document.read(cx).recovery_key();
                            this.retire_clean_document_recovery(id, current_key, cx);
                        }
                        cx.notify();
                    }
                    DocumentEvent::ScrollWebPreview(fraction) => {
                        this.queue_web_scroll(*fraction, cx)
                    }
                    DocumentEvent::RetryWebPreview => this.retry_web_preview(document, cx),
                    DocumentEvent::WebPreviewLayoutLeft => {
                        this.web_preview_layout_left(document, cx)
                    }
                },
            ),
            // A document notifies on mode change, trust change, and after a
            // reparse — every event that can alter what the WebView should
            // show. Observing is what replaces the old sync-from-render.
            cx.observe(&view, |this, _, cx| {
                this.web_dirty(cx);
            }),
        ];

        let needs_recovery = view.read(cx).is_dirty();
        let recovery_document = view.clone();
        self.tabs.push(
            identity.clone(),
            DocumentTab {
                view,
                _subscriptions: subscriptions,
            },
        );
        // Recovered buffers are already dirty when their tab is inserted, so
        // they have no subsequent editor event that could arm recovery.
        if arm_dirty_recovery && needs_recovery {
            self.arm_document_recovery(&recovery_document, cx);
        }
        if let Some(path) = identity.path() {
            self.record_visit(path.to_path_buf(), 0);
        }
        self.sync_document_watches(cx);
        self.web_dirty(cx);
        cx.notify();
    }

    fn lifecycle_documents(&self, cx: &App) -> Vec<DocumentLifecycle> {
        self.tabs
            .iter()
            .map(|tab| {
                let document = tab.payload.view.read(cx);
                DocumentLifecycle {
                    id: document.id(),
                    dirty: document.is_dirty(),
                    snapshot: document.source_snapshot(cx),
                }
            })
            .collect()
    }

    fn document_index(
        &self,
        id: mt_core::document::lifecycle::DocumentId,
        cx: &App,
    ) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.payload.view.read(cx).id() == id)
    }

    fn document_by_id(
        &self,
        id: mt_core::document::lifecycle::DocumentId,
        cx: &App,
    ) -> Option<Entity<DocumentView>> {
        self.document_index(id, cx)
            .and_then(|ix| self.document_at(ix).cloned())
    }

    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.document_at(ix).map(|document| document.read(cx).id()) else {
            return;
        };
        let action = DestructiveAction::CloseTab(id);
        if self.request_destructive(action.clone(), window, cx) {
            self.perform_destructive(action, window, cx);
        }
    }

    fn destructive_action_has_revision_answers(
        &self,
        action: &DestructiveAction,
        cx: &App,
    ) -> bool {
        match action {
            DestructiveAction::CloseTab(id) => {
                self.revision_has_authored_answers_for_document(*id, cx)
            }
            DestructiveAction::CloseWindow | DestructiveAction::ReplaceWorkspace(_) => {
                self.review_flow.revision_has_authored_answers()
            }
        }
    }

    /// Keep the platform window alive long enough to release a focused input
    /// handler before the application exits.
    fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.window_close_ready {
            return true;
        }
        if self.window_close_pending {
            return false;
        }
        if self.request_destructive(DestructiveAction::CloseWindow, window, cx) {
            self.close_window_after_input_drain(window, cx);
        }
        false
    }

    fn close_window_after_input_drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.window_close_pending {
            return;
        }
        self.cancel_pending_revision(cx);
        self.window_close_pending = true;
        window.disable_focus(cx);
        let workspace = cx.entity().downgrade();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                let ready = workspace
                    .update(cx, |workspace, _| workspace.window_close_ready = true)
                    .is_ok();
                if ready {
                    Self::post_native_window_close(window);
                } else {
                    window.remove_window();
                }
            });
        });
    }

    #[cfg(target_os = "windows")]
    fn post_native_window_close(window: &mut Window) {
        use raw_window_handle::RawWindowHandle;
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

        let hwnd = raw_window_handle::HasWindowHandle::window_handle(window)
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
                _ => None,
            });
        let Some(hwnd) = hwnd else {
            window.remove_window();
            return;
        };
        if unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_err() {
            window.remove_window();
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn post_native_window_close(window: &mut Window) {
        window.remove_window();
    }

    fn request_workspace_replace(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = DestructiveAction::ReplaceWorkspace(path);
        if self.request_destructive(action.clone(), window, cx) {
            self.perform_destructive(action, window, cx);
        }
    }

    /// Start one Save / Discard / Cancel walk. The return value means the
    /// action has no dirty documents and can proceed synchronously.
    fn request_destructive(
        &mut self,
        action: DestructiveAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.destructive_action_has_revision_answers(&action, cx) {
            self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
            return false;
        }
        if self.pending_destructive.is_some() || self.pending_startup_destructive.is_some() {
            return false;
        }
        self.pending_destructive_recovery.clear();
        let keys = self.pending_recovery_keys(&action);
        let request = DestructiveRequest::new(action, &self.lifecycle_documents(cx));
        match request.initial_resolution() {
            DestructiveResolution::Proceed(action) => {
                if keys.is_empty() {
                    true
                } else {
                    self.perform_after_discard_retirement(request, action, keys, window, cx);
                    false
                }
            }
            DestructiveResolution::Prompt(_) => {
                self.pending_destructive = Some(request);
                self.prompt_destructive(window, cx);
                false
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => false,
        }
    }

    fn prompt_destructive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .pending_destructive
            .as_ref()
            .and_then(DestructiveRequest::current)
        else {
            return;
        };
        let Some(document) = self.document_by_id(id, cx) else {
            self.resolve_destructive(DirtyDecision::Discard, window, cx);
            return;
        };
        let title = document.read(cx).title(cx);
        let message = format!("Save changes to {title}?");
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Your changes will be lost if you discard them."),
            &[
                PromptButton::ok("Save"),
                PromptButton::new("Discard"),
                PromptButton::cancel("Cancel"),
            ],
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(2);
            let decision = match answer {
                0 => DirtyDecision::Save,
                1 => DirtyDecision::Discard,
                _ => DirtyDecision::Cancel,
            };
            loop {
                if crate::views::try_update_in(&this, cx, |this, window, cx| {
                    this.resolve_destructive(decision, window, cx);
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn resolve_destructive(
        &mut self,
        decision: DirtyDecision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut request) = self.pending_destructive.take() else {
            return;
        };
        let documents = self.lifecycle_documents(cx);
        if !request.current_prompt_matches(&documents) {
            match request.revalidate(&documents) {
                DestructiveResolution::Prompt(_) => {
                    self.pending_destructive = Some(request);
                    self.prompt_destructive(window, cx);
                }
                DestructiveResolution::Proceed(action) => {
                    let keys = std::mem::take(&mut self.pending_destructive_recovery);
                    self.perform_after_discard_retirement(request, action, keys, window, cx);
                }
                DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {}
            }
            return;
        }
        let current = request.current();
        let current_document = current.and_then(|id| self.document_by_id(id, cx));
        let recovery_key = current_document
            .as_ref()
            .map(|document| document.read(cx).recovery_key());
        let snapshot_before_save = current_document
            .as_ref()
            .map(|document| document.read(cx).source_snapshot(cx));
        if decision == DirtyDecision::Save
            && let Some(document) = current_document.as_ref()
            && !document.read(cx).is_on_disk()
        {
            // A memory buffer has no normal Save destination. Keep the exact
            // request alive until Save As writes the snapshot the user chose.
            self.pending_destructive = Some(request);
            self.prompt_destructive_save_as(document.read(cx).id(), window, cx);
            return;
        }
        let save_succeeded = current_document.is_some_and(|document| {
            decision == DirtyDecision::Save
                && document.update(cx, |document, cx| document.save(SaveMode::Normal, cx))
        });
        let saved_snapshot = save_succeeded.then_some(snapshot_before_save).flatten();
        let resolution = request.decide(decision, saved_snapshot, &self.lifecycle_documents(cx));
        if let (Some(id), Some(key)) = (current, recovery_key)
            && matches!(decision, DirtyDecision::Save | DirtyDecision::Discard)
            && !matches!(
                resolution,
                DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_)
            )
        {
            self.pending_destructive_recovery.push((key, Some(id)));
        }
        match resolution {
            DestructiveResolution::Prompt(_) => {
                self.pending_destructive = Some(request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(_action) => {
                // The final scan is immediately before destruction. This is
                // needed because another document can become dirty while a
                // previous document's modal prompt is open.
                match request.revalidate(&self.lifecycle_documents(cx)) {
                    DestructiveResolution::Prompt(_) => {
                        self.pending_destructive = Some(request);
                        self.prompt_destructive(window, cx);
                    }
                    DestructiveResolution::Proceed(action) => {
                        let keys = std::mem::take(&mut self.pending_destructive_recovery);
                        self.perform_after_discard_retirement(request, action, keys, window, cx);
                    }
                    DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {}
                }
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.pending_destructive_recovery.clear();
            }
        }
    }

    fn save_as_snapshot_is_current(
        &self,
        id: mt_core::document::lifecycle::DocumentId,
        cx: &App,
    ) -> bool {
        match self.pending_destructive.as_ref() {
            None => true,
            Some(request) => {
                request.current() == Some(id)
                    && request.current_prompt_matches(&self.lifecycle_documents(cx))
            }
        }
    }

    fn cancel_pending_destructive_save_as(&mut self, id: mt_core::document::lifecycle::DocumentId) {
        if self
            .pending_destructive
            .as_ref()
            .is_some_and(|request| request.current() == Some(id))
        {
            self.pending_destructive = None;
            self.pending_destructive_recovery.clear();
        }
    }

    fn complete_pending_destructive_save_as(
        &mut self,
        id: mt_core::document::lifecycle::DocumentId,
        saved_snapshot: mt_core::document::lifecycle::BufferSnapshot,
        recovery_key: RecoveryKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut request) = self.pending_destructive.take() else {
            return;
        };
        if request.current() != Some(id) {
            self.pending_destructive = Some(request);
            return;
        }
        let resolution = request.decide(
            DirtyDecision::Save,
            Some(saved_snapshot),
            &self.lifecycle_documents(cx),
        );
        if !matches!(
            resolution,
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_)
        ) {
            self.pending_destructive_recovery
                .push((recovery_key, Some(id)));
        }
        match resolution {
            DestructiveResolution::Prompt(_) => {
                self.pending_destructive = Some(request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(_action) => {
                match request.revalidate(&self.lifecycle_documents(cx)) {
                    DestructiveResolution::Prompt(_) => {
                        self.pending_destructive = Some(request);
                        self.prompt_destructive(window, cx);
                    }
                    DestructiveResolution::Proceed(action) => {
                        let keys = std::mem::take(&mut self.pending_destructive_recovery);
                        self.perform_after_discard_retirement(request, action, keys, window, cx);
                    }
                    DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                        self.pending_destructive_recovery.clear();
                    }
                }
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.pending_destructive_recovery.clear();
            }
        }
    }

    fn perform_destructive(
        &mut self,
        action: DestructiveAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            DestructiveAction::CloseTab(id) => {
                if let Some(ix) = self.document_index(id, cx) {
                    self.close_tab_unchecked(ix, cx);
                }
            }
            DestructiveAction::CloseWindow => self.close_window_after_input_drain(window, cx),
            DestructiveAction::ReplaceWorkspace(path) => {
                while !self.tabs.is_empty() {
                    self.close_tab_unchecked(self.tabs.len() - 1, cx);
                }
                self.open_folder(path.clone(), window, cx);
                if self.root.as_deref() == Some(path.as_path()) {
                    self.record_recent_workspace(path, cx);
                }
            }
        }
    }

    fn close_tab_unchecked(&mut self, ix: usize, cx: &mut Context<Self>) {
        // `Tabs::close` shifts the active index, empties the preview slot if it
        // named this tab, and drops the tab's subscriptions with it. Callers
        // reach this only after the destructive interlock has granted access.
        let recovery_id = self.document_at(ix).map(|document| document.read(cx).id());
        if let Some(id) = recovery_id {
            if let Some(request) = self
                .pending_save_as
                .filter(|request| request.document_id == id)
            {
                self.pending_save_as = None;
                if request.origin == SaveAsRequestOrigin::Destructive {
                    self.pending_destructive_recovery.clear();
                }
            }
            // A close is the last intentional lifecycle decision for this
            // buffer. Cancel its deadline before invalidating any checkpoint
            // capability held by an already-running worker.
            self.retire_document_recovery(id, None, cx);
            self.cancel_pending_review_for_document(id, cx);
            self.cancel_pending_revision_for_document(id, cx);
            self.review_flow.remove_document_lens_override(id);
            self.review_flow.remove_recovered_revision_document(id);
        }
        let Some((closed, _dropped)) = self.tabs.close(ix) else {
            return;
        };
        // Otherwise Back reopens the tab that was just closed, which reads as
        // the close button not working.
        if let Some(path) = closed.path() {
            self.history.forget(path);
        }
        self.sync_document_watches(cx);
        self.web_dirty(cx);
        cx.notify();
    }

    /// Redraw everything after the interface language changed.
    ///
    /// Labels are resolved from the string table during render, so nothing is
    /// cached — but a view only redraws when it is notified, and the panels are
    /// separate entities that did not observe the settings change.
    fn relabel(&mut self, cx: &mut Context<Self>) {
        if let Some(explorer) = &self.explorer {
            explorer.update(cx, |_, cx| cx.notify());
        }
        if let Some(harness) = &self.harness {
            harness.update(cx, |_, cx| cx.notify());
        }
        // The search view resolves its own labels through `i18n::t` at render
        // time — its scope names, its "no matches" line — and it was missing
        // from this list, so switching language left that one panel in the old
        // one until something else happened to redraw it.
        self.search.update(cx, |_, cx| cx.notify());
        for doc in self.document_views() {
            doc.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Rediscover skills, e.g. after a setting changed what is in scope.
    fn rescan_harness(&mut self, cx: &mut Context<Self>) {
        if let Some(harness) = &self.harness {
            harness.update(cx, |harness, cx| harness.refresh(cx));
        }
    }

    /// Re-resolve the saved theme and repaint everything that caches it.
    ///
    /// The Web preview renders in its own browser context and caches its HTML
    /// with the palette baked in, so it does not pick up a GPUI theme change on
    /// its own. Called for both a mode change and a preset change — the two are
    /// separate settings that land in the same place, and the settings page has
    /// already written whichever one moved.
    fn reapply_theme(&mut self, cx: &mut Context<Self>) {
        let preference = crate::settings::AppSettings::global(cx).theme;
        crate::settings::apply_theme(preference, None, cx);
        for doc in self.document_views() {
            doc.update(cx, |doc, cx| doc.theme_changed(cx));
        }
        self.web_dirty(cx);
        cx.notify();
    }

    fn active_document(&self) -> Option<&Entity<DocumentView>> {
        self.document_at(self.tabs.active_index())
    }

    fn web_active(&self, cx: &App) -> bool {
        !self.settings_open
            && self
                .active_document()
                .is_some_and(|document| document.read(cx).layout().uses_webview())
    }

    fn workspace_panel_widths(
        &self,
        viewport: Pixels,
        right_visible: bool,
    ) -> WorkspacePanelWidths {
        resolved_workspace_panel_widths(
            self.preferred_left_panel_width,
            self.preferred_right_panel_width,
            self.left_panel_open,
            right_visible,
            viewport,
        )
    }

    fn on_panel_resize_drag(
        &mut self,
        event: &DragMoveEvent<WorkspaceResizeEdge>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edge = *event.drag(cx);
        let Some(grab) = self.panel_resize_grab.filter(|grab| grab.edge == edge) else {
            return;
        };
        let viewport = self.layout_width.unwrap_or(window.viewport_size().width);
        let boundary = (event.event.position.x - grab.pointer_offset).clamp(px(0.), viewport);
        let requested = match edge {
            WorkspaceResizeEdge::Left => boundary,
            WorkspaceResizeEdge::Right => viewport - boundary,
        };
        self.set_panel_width(edge, requested, window, cx);
    }

    fn resize_panel_by_delta(
        &mut self,
        edge: WorkspaceResizeEdge,
        delta: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.layout_width.unwrap_or(window.viewport_size().width);
        let right_visible = self.right_panel_open;
        let widths = self.workspace_panel_widths(viewport, right_visible);
        let current = match edge {
            WorkspaceResizeEdge::Left => widths.left,
            WorkspaceResizeEdge::Right => widths.right,
        };
        self.set_panel_width(edge, current + delta, window, cx);
    }

    fn set_panel_width(
        &mut self,
        edge: WorkspaceResizeEdge,
        requested: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.layout_width.unwrap_or(window.viewport_size().width);
        let right_visible = self.right_panel_open;
        let widths = self.workspace_panel_widths(viewport, right_visible);
        let width = match edge {
            WorkspaceResizeEdge::Left => {
                clamped_dragged_panel_width(edge, requested, widths.right, right_visible, viewport)
            }
            WorkspaceResizeEdge::Right => clamped_dragged_panel_width(
                edge,
                requested,
                widths.left,
                self.left_panel_open,
                viewport,
            ),
        };
        let preferred = match edge {
            WorkspaceResizeEdge::Left => &mut self.preferred_left_panel_width,
            WorkspaceResizeEdge::Right => &mut self.preferred_right_panel_width,
        };
        if *preferred == width {
            return;
        }
        *preferred = width;
        self.web_dirty(cx);
        cx.notify();
    }

    fn on_panel_resize_key_down(
        &mut self,
        edge: WorkspaceResizeEdge,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = metrics::gap_group();
        let delta = match (edge, event.keystroke.key.as_str()) {
            (WorkspaceResizeEdge::Left, "left") | (WorkspaceResizeEdge::Right, "right") => -step,
            (WorkspaceResizeEdge::Left, "right") | (WorkspaceResizeEdge::Right, "left") => step,
            _ => return,
        };
        window.prevent_default();
        cx.stop_propagation();
        self.resize_panel_by_delta(edge, delta, window, cx);
    }

    fn set_status(&mut self, message: String, cx: &mut Context<Self>) {
        // Each message gets its own generation, and only the timer whose
        // generation is still current clears the bar. Without it, two messages
        // inside the window meant the first message's timer wiped the second
        // one off the screen early — and every message spawned a task that
        // outlived its own relevance.
        self.status_generation = self.status_generation.wrapping_add(1);
        let generation = self.status_generation;
        self.status = Some(message);
        cx.notify();
        // Clear after a few seconds so the bar does not hold a stale message.
        self._status_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STATUS_LINGER).await;
            crate::views::try_update(&this, cx, |this, cx| {
                if this.status_generation == generation {
                    this.status = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Apply pending filesystem changes.
    fn drain_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(watcher) = &self.watcher else { return };
        let root = watcher.root().to_path_buf();
        let changes = watcher.poll();
        self.apply_watcher_changes(&root, &changes, cx);
    }

    /// Apply already-classified filesystem events. Keeping this separate from
    /// polling makes the document safety decision deterministic: the watcher
    /// owns OS delivery timing, while the workspace owns what a change means.
    fn apply_watcher_changes(
        &mut self,
        watcher_root: &Path,
        changes: &[Change],
        cx: &mut Context<Self>,
    ) {
        if changes.is_empty() {
            return;
        }

        // The editor snapshot covers `SKILL.md`; an Agent Skill Review also
        // owns every supporting package path. A watcher signal under that root
        // makes the frozen result stale immediately, without rereading files
        // from render or risking a current-looking result after an edit.
        self.review_flow.observe_supporting_source_changes(changes);
        let context_changed = self
            .harness
            .as_ref()
            .and_then(|harness| harness.read(cx).resolved_context())
            .is_some_and(|context| {
                let paths = context.watch_paths();
                changes.iter().any(|change| {
                    paths.iter().any(|path| {
                        #[cfg(windows)]
                        {
                            // Deleted paths must match without canonicalizing them.
                            let path = path.to_string_lossy().replace('\\', "/");
                            let changed = change.path().to_string_lossy().replace('\\', "/");
                            let path = path
                                .strip_prefix("//?/")
                                .or_else(|| path.strip_prefix("//./"))
                                .unwrap_or(&path)
                                .trim_end_matches('/');
                            let changed = changed
                                .strip_prefix("//?/")
                                .or_else(|| changed.strip_prefix("//./"))
                                .unwrap_or(&changed)
                                .trim_end_matches('/');
                            path.eq_ignore_ascii_case(changed)
                                || change.affects_tree()
                                    && path
                                        .get(..changed.len())
                                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(changed))
                                    && path.as_bytes().get(changed.len()) == Some(&b'/')
                        }
                        #[cfg(not(windows))]
                        {
                            path == change.path()
                                || change.affects_tree() && path.starts_with(change.path())
                        }
                    })
                })
            });
        if context_changed {
            let input = self
                .harness
                .as_ref()
                .and_then(|harness| harness.read(cx).context_input().cloned());
            self.resolve_effective_context(input, cx);
        }

        let tree_changed = changes
            .iter()
            .any(|change| change.affects_tree() && change.path().starts_with(watcher_root));
        let removed_artifact = self.harness.as_ref().is_some_and(|harness| {
            let harness = harness.read(cx);
            changes
                .iter()
                .any(|change| change.affects_tree() && harness.has_artifact_under(change.path()))
        });
        let harness_changed = removed_artifact
            || changes
                .iter()
                .any(|change| path_affects_harness(watcher_root, change.path()));

        // Every open document whose file changed. With auto-reload off, the
        // flag and its banner are the whole response — the user's unsaved edits
        // are theirs to keep or discard. With it on, a *clean* document is
        // re-read; a dirty one is not, and `reload_if_clean` saying so is
        // exactly the signal that the banner is still needed. Automatic refresh
        // must never discard typed text.
        //
        // `reload_if_clean` returns whether it *started*: the read and the parse
        // run on a background task, because markdown-rs is superlinear and this
        // fires on every external write. A document that goes dirty while that
        // parse runs is flagged by the task itself when the result lands.
        let auto_reload = !self.is_startup_recovery_pending()
            && crate::settings::AppSettings::global(cx).watch_auto_reload;
        // Cloned: `self.documents` cannot stay borrowed across the `&mut cx`
        // that leasing each entity takes. Coalescing matching events avoids
        // duplicate reloads in one poll; writes delivered later remain queued
        // for the next poll.
        let documents = self.document_views();
        for doc in documents {
            let affected = {
                let document = doc.read(cx);
                changes
                    .iter()
                    .any(|change| document.watches_path(change.path()))
            };
            if !affected {
                continue;
            }
            let reloaded = auto_reload && doc.update(cx, |doc, cx| doc.reload_if_clean(cx));
            if !reloaded {
                doc.update(cx, |doc, cx| doc.mark_externally_changed(cx));
            }
        }

        if tree_changed && let Some(explorer) = &self.explorer {
            explorer.update(cx, |explorer, cx| explorer.refresh(cx));
        }
        if harness_changed && let Some(harness) = &self.harness {
            harness.update(cx, |harness, cx| harness.refresh(cx));
        }
        cx.notify();
    }

    // --- Actions ----------------------------------------------------------

    fn on_new_document(&mut self, _: &NewDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.new_memory(String::new(), window, cx);
    }

    fn on_paste_into_new(&mut self, _: &PasteIntoNew, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_status(i18n::t(i18n::Key::ClipboardTextUnavailable, cx).into(), cx);
            return;
        };
        self.new_memory(text, window, cx);
    }

    fn on_open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t(i18n::Key::OpenFile, cx).into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = paths
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.first().cloned())
            else {
                return;
            };
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                this.open_target(path, true, window, cx);
            });
        })
        .detach();
    }

    fn on_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(i18n::t(i18n::Key::OpenFolder, cx).into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // The prompt future nests: cancelled -> failed -> no selection.
            let Some(path) = paths
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.first().cloned())
            else {
                return;
            };
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                this.open_target(path, true, window, cx);
            });
        })
        .detach();
    }

    fn on_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_document().cloned() {
            let id = doc.read(cx).id();
            if doc.update(cx, |doc, cx| doc.save(SaveMode::Normal, cx)) {
                self.clear_revision_after_save(id, cx);
            }
        }
    }

    fn on_save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(document) = self.active_document() {
            self.prompt_save_as(document.read(cx).id(), window, cx);
        }
    }

    fn prompt_save_as(
        &mut self,
        id: mt_core::document::lifecycle::DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_save_as_picker(id, SaveAsRequestOrigin::Normal, None, window, cx);
    }

    fn prompt_destructive_save_as(
        &mut self,
        id: mt_core::document::lifecycle::DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_save_as_picker(id, SaveAsRequestOrigin::Destructive, None, window, cx);
    }

    fn start_save_as_picker(
        &mut self,
        id: DocumentId,
        origin: SaveAsRequestOrigin,
        revision_approval: Option<RevisionSaveAsApproval>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.document_by_id(id, cx) else {
            return;
        };
        let ticket = self.begin_save_as_request(id, origin, revision_approval);
        let source = document.read(cx).source_path().map(Path::to_path_buf);
        let directory = source
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.root.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = source
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled.md")
            .to_string();
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn_in(window, async move |this, cx| {
            let path = path.await.ok().and_then(Result::ok).flatten();
            loop {
                if crate::views::try_update_in(&this, cx, |this, window, cx| {
                    this.finish_save_as_selection_for_request(id, ticket, path.clone(), window, cx);
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn begin_save_as_request(
        &mut self,
        document_id: DocumentId,
        origin: SaveAsRequestOrigin,
        revision_approval: Option<RevisionSaveAsApproval>,
    ) -> u64 {
        if let Some(previous) = self.pending_save_as
            && previous.origin == SaveAsRequestOrigin::Destructive
            && origin != SaveAsRequestOrigin::Destructive
        {
            self.cancel_pending_destructive_save_as(previous.document_id);
        }
        self.save_as_request_generation = self.save_as_request_generation.wrapping_add(1);
        let ticket = self.save_as_request_generation;
        self.pending_save_as = Some(PendingSaveAsRequest {
            ticket,
            document_id,
            origin,
            revision_approval,
        });
        ticket
    }

    fn save_as_request_is_current(&self, document_id: DocumentId, ticket: u64) -> bool {
        self.pending_save_as
            .is_some_and(|request| request.document_id == document_id && request.ticket == ticket)
    }

    fn current_save_as_request(&self, document_id: DocumentId) -> Option<PendingSaveAsRequest> {
        self.pending_save_as
            .filter(|request| request.document_id == document_id)
    }

    fn clear_save_as_request(&mut self, document_id: DocumentId, ticket: u64) {
        if self.save_as_request_is_current(document_id, ticket) {
            self.pending_save_as = None;
        }
    }

    fn cancel_save_as_request(&mut self, document_id: DocumentId, ticket: u64) {
        let Some(request) = self
            .pending_save_as
            .filter(|request| request.document_id == document_id && request.ticket == ticket)
        else {
            return;
        };
        self.pending_save_as = None;
        if request.origin == SaveAsRequestOrigin::Destructive {
            self.cancel_pending_destructive_save_as(document_id);
        }
    }

    #[cfg(test)]
    fn finish_save_as_selection(
        &mut self,
        id: mt_core::document::lifecycle::DocumentId,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match path {
            Some(path) => self.finish_save_as(id, path, SaveAsMode::CreateOnly, window, cx),
            None => {
                if let Some(request) = self.current_save_as_request(id) {
                    self.cancel_save_as_request(id, request.ticket);
                } else {
                    self.cancel_pending_destructive_save_as(id);
                }
            }
        }
    }

    fn finish_save_as_selection_for_request(
        &mut self,
        id: DocumentId,
        ticket: u64,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save_as_request_is_current(id, ticket) {
            return;
        }
        match path {
            Some(path) => self.finish_save_as_for_request(
                id,
                ticket,
                path,
                SaveAsMode::CreateOnly,
                window,
                cx,
            ),
            None => self.cancel_save_as_request(id, ticket),
        }
    }

    fn prompt_save_as_overwrite(
        &mut self,
        id: DocumentId,
        ticket: u64,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save_as_request_is_current(id, ticket) {
            return;
        }
        let authorization = match fs::SaveAsOverwriteAuthorization::capture(&path) {
            Ok(authorization) => Arc::new(authorization),
            Err(error) => {
                self.cancel_save_as_request(id, ticket);
                self.set_status(format!("Save As failed: {error}"), cx);
                return;
            }
        };
        let title = i18n::replace_file_title(&path, cx);
        let description = i18n::replace_file_description(&path, cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&description),
            &[
                PromptButton::ok(i18n::t(i18n::Key::Replace, cx)),
                PromptButton::cancel(i18n::t(i18n::Key::Cancel, cx)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let replace = answer.await.unwrap_or(1) == 0;
            loop {
                if crate::views::try_update_in(&this, cx, |this, window, cx| {
                    if !this.save_as_request_is_current(id, ticket) {
                        return;
                    }
                    if replace {
                        this.finish_save_as_for_request(
                            id,
                            ticket,
                            path.clone(),
                            SaveAsMode::Overwrite(authorization.clone()),
                            window,
                            cx,
                        );
                    } else {
                        this.cancel_save_as_request(id, ticket);
                    }
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    #[cfg(test)]
    fn finish_save_as(
        &mut self,
        id: DocumentId,
        path: PathBuf,
        mode: SaveAsMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ticket = self.current_save_as_request(id).map_or_else(
            || {
                let origin = if self
                    .pending_destructive
                    .as_ref()
                    .and_then(DestructiveRequest::current)
                    == Some(id)
                {
                    SaveAsRequestOrigin::Destructive
                } else {
                    SaveAsRequestOrigin::Normal
                };
                self.begin_save_as_request(id, origin, None)
            },
            |request| request.ticket,
        );
        self.finish_save_as_for_request(id, ticket, path, mode, window, cx);
    }

    fn finish_save_as_for_request(
        &mut self,
        id: DocumentId,
        ticket: u64,
        path: PathBuf,
        mode: SaveAsMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save_as_request_is_current(id, ticket) {
            return;
        }
        let request = self
            .current_save_as_request(id)
            .expect("the Save As ticket must remain current during a UI update");
        let Some(ix) = self.document_index(id, cx) else {
            self.cancel_save_as_request(id, ticket);
            return;
        };
        if !self.save_as_snapshot_is_current(id, cx) {
            self.cancel_save_as_request(id, ticket);
            self.set_status(i18n::save_as_snapshot_changed_message(cx).into(), cx);
            return;
        }
        if self.tabs.iter().enumerate().any(|(existing, tab)| {
            existing != ix
                && tab
                    .path()
                    .is_some_and(|candidate| fs::paths_match(candidate, &path))
        }) {
            self.cancel_save_as_request(id, ticket);
            self.set_status(i18n::save_as_path_already_open_message(&path, cx), cx);
            return;
        }
        let document = self.document_at(ix).cloned().expect("index was found");
        let (old_path, old_recovery_key, source_was_dirty, source_was_conflicted, saved_snapshot) = {
            let document = document.read(cx);
            (
                document.source_path().map(Path::to_path_buf),
                document.recovery_key(),
                document.is_dirty(),
                document.is_externally_changed(),
                (request.origin == SaveAsRequestOrigin::Destructive)
                    .then(|| document.source_snapshot(cx)),
            )
        };
        let create_only = mode == SaveAsMode::CreateOnly;
        if !self.save_as_request_is_current(id, ticket) {
            return;
        }
        if request.origin == SaveAsRequestOrigin::Revision
            && !self.revision_save_as_approval_is_current(request, cx)
        {
            self.reject_stale_revision_save_as(request, cx);
            return;
        }
        if !self.should_preserve_unreconciled_startup_recovery(
            id,
            source_was_dirty,
            source_was_conflicted,
        ) {
            self.remember_save_as_recovery_key(id, old_recovery_key.clone());
        }
        match document.update(cx, |document, cx| document.save_as(&path, mode, cx)) {
            SaveAsOutcome::Saved => {
                self.clear_save_as_request(id, ticket);
                self.tabs
                    .replace_identity(ix, TabIdentity::File(path.clone()));
                if let Some(old_path) = old_path {
                    self.history.forget(&old_path);
                } else if self.root.is_none()
                    && let Some(parent) = path.parent()
                {
                    // A first Save As gives a memory buffer its ordinary
                    // workspace identity without adding the parent as a
                    // separately opened recent workspace.
                    self.open_folder(parent.to_path_buf(), window, cx);
                }
                self.sync_document_watches(cx);
                self.record_recent_file(path.clone(), cx);
                self.record_visit(path, 0);
                self.web_dirty(cx);
                self.clear_revision_after_save(id, cx);
                if request.origin == SaveAsRequestOrigin::Destructive {
                    self.complete_pending_destructive_save_as(
                        id,
                        saved_snapshot.expect("destructive Save As captures its close snapshot"),
                        old_recovery_key,
                        window,
                        cx,
                    );
                }
                cx.notify();
            }
            SaveAsOutcome::DestinationExists if create_only => {
                self.forget_save_as_recovery_key(id);
                self.prompt_save_as_overwrite(id, ticket, path, window, cx);
            }
            SaveAsOutcome::DestinationExists | SaveAsOutcome::Failed => {
                self.forget_save_as_recovery_key(id);
                self.cancel_save_as_request(id, ticket);
            }
        }
    }

    fn revision_save_as_approval_is_current(
        &mut self,
        request: PendingSaveAsRequest,
        cx: &Context<Self>,
    ) -> bool {
        let Some(approval) = request.revision_approval else {
            return false;
        };
        if request.origin != SaveAsRequestOrigin::Revision {
            return false;
        }
        let Some(document) = self.document_by_id(request.document_id, cx) else {
            return false;
        };
        document.read(cx).source_stamp() == approval.source_stamp
            && self
                .review_flow
                .revision_save_as_approval_is_current(request.document_id, approval)
            && self.revision_applied_state_is_current_for_document(request.document_id, cx)
    }

    fn reject_stale_revision_save_as(
        &mut self,
        request: PendingSaveAsRequest,
        cx: &mut Context<Self>,
    ) {
        self.clear_save_as_request(request.document_id, request.ticket);
        if let Some(approval) = request.revision_approval
            && self
                .review_flow
                .reject_stale_revision_save_as(request.document_id, approval)
        {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
        }
    }

    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close_tab(self.tabs.active_index(), window, cx);
    }

    fn on_open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        // The WebView's visibility depends on this flag, and it is an OS child
        // window that will not notice a re-render on its own.
        self.web_dirty(cx);
        cx.notify();
    }

    fn on_toggle_left_panel(
        &mut self,
        _: &ToggleLeftPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_welcome {
            return;
        }
        self.left_panel_open = !self.left_panel_open;
        // The WebView is an OS child window; it does not notice the document
        // pane resizing under it.
        self.web_dirty(cx);
        cx.notify();
    }

    fn on_toggle_right_panel(
        &mut self,
        _: &ToggleRightPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_welcome {
            return;
        }
        self.right_panel_open = !self.right_panel_open;
        self.web_dirty(cx);
        cx.notify();
    }

    /// Open files dropped onto the window.
    ///
    /// A directory becomes the workspace; documents open as pinned tabs, since
    /// dragging a file in is as deliberate as double-clicking one. Anything the
    /// document pipeline cannot show is reported rather than silently ignored —
    /// a drop that appears to do nothing reads as a broken window.
    fn on_drop_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let mut opened = 0usize;
        let mut skipped: Vec<String> = Vec::new();

        for path in paths {
            if path.is_dir() {
                self.open_target(path.clone(), true, window, cx);
                opened += 1;
                break;
            } else if mt_core::workspace::is_openable(path) {
                opened += usize::from(self.open_target(path.clone(), true, window, cx));
            } else {
                skipped.push(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }

        if opened == 0 && !skipped.is_empty() {
            self.set_status(format!("Cannot open {}", skipped.join(", ")), cx);
        }
    }

    /// The path of whichever tab the context menu belongs to.
    ///
    /// Falls back to the active tab: the menu is also reachable by keybinding,
    /// where no tab was right-clicked. [`Tabs`] is what makes that fallback
    /// real — it drops the recorded index when the menu closes and when the tab
    /// list changes, so a stale index can never answer here.
    fn menu_target(&self, cx: &App) -> Option<PathBuf> {
        self.tabs.menu_target().and_then(|tab| {
            tab.path().map(Path::to_path_buf).or_else(|| {
                matches!(&tab.identity, TabIdentity::Recovered(_))
                    .then(|| {
                        tab.payload
                            .view
                            .read(cx)
                            .source_path()
                            .map(Path::to_path_buf)
                    })
                    .flatten()
            })
        })
    }

    fn on_copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.menu_target(cx) else {
            return;
        };
        // The menu is done with; releasing it restores the keyboard fallback to
        // the active tab. Without this the recorded index outlives its menu and
        // every later keyboard Copy Path acts on whichever tab was last
        // right-clicked, which is what `menu_target` documents it will not do.
        self.tabs.clear_menu();
        let text = path.to_string_lossy().replace(char::from(92), "/");
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.set_status(format!("Copied {text}"), cx);
    }

    fn on_copy_relative_path(
        &mut self,
        _: &CopyRelativePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.menu_target(cx) else {
            return;
        };
        self.tabs.clear_menu();
        // Without a folder open there is nothing to be relative *to*, so this
        // reports rather than silently copying the absolute path — which would
        // look like the other menu item misbehaving.
        let Some(root) = self.root.clone() else {
            self.set_status("No folder is open, so there is no relative path".into(), cx);
            return;
        };
        let Ok(rest) = path.strip_prefix(&root) else {
            self.set_status("That file is outside the open folder".into(), cx);
            return;
        };
        let text = rest.to_string_lossy().replace(char::from(92), "/");
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.set_status(format!("Copied {text}"), cx);
    }

    fn on_translate_document(
        &mut self,
        _: &TranslateDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The button goes inert while `translating`, but the keybinding does
        // not — and two requests over the same text race to overwrite the
        // editor with two different answers.
        if self.translating {
            return;
        }
        self.translate(Scope::Document, window, cx);
    }

    fn on_translate_selection(
        &mut self,
        _: &TranslateSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.translating {
            return;
        }
        let Some(doc) = self.active_document() else {
            return;
        };
        let range = doc.read(cx).selection(cx);
        if range.is_empty() {
            self.set_status("Select some text first".into(), cx);
            return;
        }
        self.translate(Scope::Selection(range), window, cx);
    }

    fn on_translate_block(
        &mut self,
        _: &TranslateBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.translating {
            return;
        }
        let Some(doc) = self.active_document() else {
            return;
        };
        let cursor = doc.read(cx).cursor(cx);
        self.translate(Scope::Block(cursor), window, cx);
    }

    fn recovered_revision_answers_for_active_document(
        &self,
        cx: &App,
    ) -> Option<(RecoveryKey, RevisionRecovery)> {
        let document = self.active_document()?.read(cx);
        self.review_flow
            .recovered_revision_answers_for_active_document(document.id(), &document.recovery_key())
    }

    fn copy_recovered_revision_answers(&mut self, cx: &mut Context<Self>) {
        let Some((key, recovery)) = self.recovered_revision_answers_for_active_document(cx) else {
            return;
        };
        let Ok(text) = export_recovered_revision_answers(&recovery) else {
            return;
        };
        let Some(document) = self.active_document().cloned() else {
            return;
        };
        let document_id = document.read(cx).id();
        if !self.begin_revision_recovery_retirement(document_id, key, recovery, cx) {
            self.set_status(
                i18n::t(i18n::Key::RevisionRecoveryRetirementFailed, cx).into(),
                cx,
            );
            return;
        }
        if document.read(cx).is_dirty()
            || self.revision_has_authored_answers_for_document(document_id, cx)
        {
            self.arm_document_recovery(&document, cx);
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.set_status(
            i18n::t(i18n::Key::RevisionRecoveredAnswersCopied, cx).into(),
            cx,
        );
    }

    fn discard_recovered_revision_answers(&mut self, cx: &mut Context<Self>) {
        let Some((key, recovery)) = self.recovered_revision_answers_for_active_document(cx) else {
            return;
        };
        if let Some(document) = self.active_document().cloned() {
            let (id, dirty) = {
                let document = document.read(cx);
                (document.id(), document.is_dirty())
            };
            if !self.begin_revision_recovery_retirement(id, key, recovery, cx) {
                self.set_status(
                    i18n::t(i18n::Key::RevisionRecoveryRetirementFailed, cx).into(),
                    cx,
                );
                return;
            }
            if dirty || self.revision_has_authored_answers_for_document(id, cx) {
                self.arm_document_recovery(&document, cx);
            }
        }
        self.set_status(i18n::t(i18n::Key::RevisionAnswersDiscarded, cx).into(), cx);
    }

    fn discard_revision_answers(&mut self, cx: &mut Context<Self>) {
        if !self.review_flow.has_revision_context()
            && self
                .recovered_revision_answers_for_active_document(cx)
                .is_some()
        {
            self.discard_recovered_revision_answers(cx);
            return;
        }
        let document = self.active_document().cloned();
        let mut retirement_started = false;
        let document_state = document.as_ref().map(|document| {
            let document = document.read(cx);
            (document.id(), document.recovery_key(), document.is_dirty())
        });
        if let Some((id, key, _)) = &document_state
            && self
                .review_flow
                .revision_context_has_authored_answers_for_document(*id)
        {
            self.retire_document_recovery(*id, Some(key.clone()), cx);
            retirement_started = self.has_durable_recovery_retirement(key);
            if !retirement_started {
                self.set_status(
                    i18n::t(i18n::Key::RevisionRecoveryRetirementFailed, cx).into(),
                    cx,
                );
                return;
            }
        }
        self.review_flow.clear_revision_answers();
        if let Some(document) = document
            && let Some((id, key, dirty)) = document_state
        {
            if !retirement_started {
                self.retire_document_recovery(id, Some(key), cx);
            }
            if dirty {
                self.arm_document_recovery(&document, cx);
            }
        }
        self.set_status(i18n::t(i18n::Key::RevisionAnswersDiscarded, cx).into(), cx);
    }

    fn clear_revision_after_save(&mut self, document_id: DocumentId, cx: &mut Context<Self>) {
        match self.review_flow.clear_revision_after_save(document_id) {
            RevisionSaveOutcome::NotApplied => return,
            RevisionSaveOutcome::AnswersRetained => {
                if let Some(document) = self.document_by_id(document_id, cx) {
                    self.arm_document_recovery(&document, cx);
                }
                self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
                return;
            }
            RevisionSaveOutcome::Cleared => {}
        }
        if let Some(document) = self.document_by_id(document_id, cx) {
            let current_key = document.read(cx).recovery_key();
            let key = self.take_recovery_key_for_retirement(document_id, current_key);
            self.retire_document_recovery(document_id, Some(key), cx);
        }
        cx.notify();
    }

    fn save_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document_id) = self
            .review_flow
            .revision_context()
            .map(|context| context.document_id)
        else {
            return;
        };
        if self
            .active_document()
            .is_none_or(|document| document.read(cx).id() != document_id)
        {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
            return;
        }
        if !self.review_flow.revision_is_applied(document_id) {
            return;
        }
        if self.revision_applied_state_is_current_for_commit(cx) {
            let Some(document) = self.document_by_id(document_id, cx) else {
                return;
            };
            if document.read(cx).is_on_disk() {
                self.on_save(&Save, window, cx);
            } else {
                // DocumentView::save would emit SaveAsRequested and lose this
                // approval's binding in the ordinary SaveAs picker.
                self.start_revision_save_as_picker(document_id, window, cx);
            }
            return;
        }
        self.review_flow.clear_revision_applied_state();
        self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
    }

    fn save_as_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document_id) = self
            .review_flow
            .revision_context()
            .map(|context| context.document_id)
        else {
            return;
        };
        if self
            .active_document()
            .is_none_or(|document| document.read(cx).id() != document_id)
        {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
            return;
        }
        if !self.review_flow.revision_is_applied(document_id) {
            return;
        }
        if self.revision_applied_state_is_current_for_commit(cx) {
            self.start_revision_save_as_picker(document_id, window, cx);
            return;
        }
        self.review_flow.clear_revision_applied_state();
        self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
    }

    fn start_revision_save_as_picker(
        &mut self,
        document_id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.document_by_id(document_id, cx) else {
            return;
        };
        let approval = self
            .review_flow
            .revision_save_as_approval(document.read(cx).source_stamp());
        self.start_save_as_picker(
            document_id,
            SaveAsRequestOrigin::Revision,
            Some(approval),
            window,
            cx,
        );
    }

    fn test_model_credential(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.translating {
            return;
        }
        let settings = crate::settings::AppSettings::global(cx).clone();
        let vault = crate::credentials::AppCredentialVault::global(cx).clone();
        let prepared = match PreparedTranslation::from_settings(&settings, &vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.set_status(format!("Credential test unavailable: {error}"), cx);
                return;
            }
        };
        let provider = prepared.provider();
        let endpoint = prepared.endpoint().normalized_identity();

        self.translating = true;
        self.set_status(
            format!("Testing {provider} credential with {endpoint}\u{2026}"),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { prepared.test_connection() })
                .await;
            let result = Arc::new(Mutex::new(Some(result)));
            loop {
                let result = result.clone();
                let endpoint = endpoint.clone();
                if crate::views::try_update_in(&this, cx, move |this, _, cx| {
                    let result = result
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    let Some(result) = result else { return };
                    this.translating = false;
                    match result {
                        Ok(()) => this.set_status(
                            format!("Credential accepted by {provider} at {endpoint}"),
                            cx,
                        ),
                        Err(error) => {
                            this.set_status(format!("Credential test failed: {error}"), cx)
                        }
                    }
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    /// Translate `scope` of the active document.
    ///
    /// Runs on a background task: a network round-trip must never block the UI
    /// thread. The document engine decides what is translatable; this only
    /// picks the provider.
    fn translate(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        // Parse the editor's *current* text rather than reusing
        // `doc.document()`: that parse is debounced by 180ms, so translating
        // right after a keystroke would translate the previous text and then
        // overwrite the editor with it — silently discarding the edit.
        let source_snapshot = doc.read(cx).async_snapshot(cx);
        let text = source_snapshot.text().to_owned();
        let doc_type = doc.read(cx).document().doc_type();
        let source = mt_core::Document::with_type(doc_type, text);
        let request = TranslationRequest::prepare(&source, &scope);
        if request.inputs().is_empty() {
            self.set_status("No translatable text in the selected scope".into(), cx);
            return;
        }

        let settings = crate::settings::AppSettings::global(cx).clone();
        let vault = crate::credentials::AppCredentialVault::global(cx).clone();
        let prepared = match PreparedTranslation::from_settings(&settings, &vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.set_status(format!("Translation unavailable: {error}"), cx);
                return;
            }
        };
        let target = settings.translate_to.trim().to_string();
        let target = if target.is_empty() {
            "zh".to_string()
        } else {
            target
        };
        let target = Arc::new(target);
        let prepared = prepared.bind_request(request);
        let provider = prepared.provider();
        let prompt_description = i18n::model_request_disclosure(prepared.disclosure(), cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            i18n::t(i18n::Key::ModelRequestConsentTitle, cx),
            Some(&prompt_description),
            &[
                PromptButton::ok(i18n::t(i18n::Key::SendToModel, cx)),
                PromptButton::cancel(i18n::t(i18n::Key::Cancel, cx)),
            ],
            cx,
        );
        let doc = doc.downgrade();

        self.set_status(i18n::t(i18n::Key::ModelRequestWaiting, cx).into(), cx);
        // Set before awaiting the prompt: keybindings must not stack multiple
        // consent dialogs over the same source snapshot.
        self.translating = true;

        cx.spawn_in(window, async move |this, cx| {
            let approved = answer.await.unwrap_or(1) == 0;
            let pending = Arc::new(Mutex::new(Some(prepared)));

            loop {
                let pending = pending.clone();
                let doc = doc.clone();
                let source_snapshot = source_snapshot.clone();
                let target = target.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let Some(prepared) = pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    else {
                        return;
                    };

                    if !approved {
                        this.translating = false;
                        this.set_status(
                            i18n::t(i18n::Key::ModelRequestCancelled, cx).into(),
                            cx,
                        );
                        return;
                    }

                    let Some(document) = doc.upgrade() else {
                        this.translating = false;
                        this.set_status(
                            i18n::t(i18n::Key::ModelRequestDocumentClosed, cx).into(),
                            cx,
                        );
                        return;
                    };
                    if document.read(cx).async_snapshot(cx) != source_snapshot {
                        this.translating = false;
                        this.set_status(
                            i18n::t(i18n::Key::ModelRequestDocumentChanged, cx).into(),
                            cx,
                        );
                        return;
                    }

                    let mut consent = ConsentCapability::from_decision(
                        prepared.disclosure(),
                        ConsentDecision::Approve,
                    );
                    let authorization = match prepared.authorize(&mut consent) {
                        Ok(authorization) => authorization,
                        Err(error) => {
                            this.translating = false;
                            this.set_status(format!("Translation unavailable: {error}"), cx);
                            return;
                        }
                    };

                    this.set_status(format!("Translating via {}…", provider.label()), cx);
                    let doc = document.downgrade();
                    cx.spawn_in(window, async move |this, cx| {
                        let result = cx
                            .background_spawn(async move {
                                prepared.execute(authorization, target.as_str())
                            })
                            .await;

                        // A fallible window borrow may skip one draw frame. Retain the
                        // result and retry so the loading flag always clears and a stale
                        // result is still reported instead of disappearing silently.
                        let result = Arc::new(Mutex::new(Some(result)));
                        loop {
                            let result = result.clone();
                            let doc = doc.clone();
                            let source_snapshot = source_snapshot.clone();
                            if crate::views::try_update_in(
                                &this,
                                cx,
                                move |this, window, cx| {
                                    let result = result
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .take();
                                    let Some(result) = result else { return };
                                    // Cleared in both arms — a flag left set by the error path
                                    // is a permanently dead button.
                                    this.translating = false;
                                    match result {
                                        Ok(translation) => {
                                            let applied = doc.upgrade().is_some_and(|doc| {
                                                doc.update(cx, |doc, cx| {
                                                    doc.replace_text_if_current(
                                                        &source_snapshot,
                                                        translation.text,
                                                        window,
                                                        cx,
                                                    )
                                                })
                                            });
                                            if applied {
                                                this.set_status(
                                                    format!(
                                                        "Translated {} segment(s) via {}",
                                                        translation
                                                            .segments
                                                            .iter()
                                                            .filter(|s| s.translatable)
                                                            .count(),
                                                        provider.label()
                                                    ),
                                                    cx,
                                                );
                                            } else {
                                                this.set_status(
                                                    "The document changed while translation was running. The result was not applied; run Translate again to use the latest text."
                                                        .into(),
                                                    cx,
                                                );
                                            }
                                        }
                                        Err(err) => this
                                            .set_status(format!("Translation failed: {err}"), cx),
                                    }
                                },
                            )
                            .is_some()
                            {
                                break;
                            }
                            if this.upgrade().is_none() {
                                break;
                            }
                            cx.background_executor()
                                .timer(Duration::from_millis(1))
                                .await;
                        }
                    })
                    .detach();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    // --- Rendering --------------------------------------------------------

    fn render_document_details(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (title, kind, location, status) = {
            let document = self.active_document()?.read(cx);
            let location = document.source_path().map_or_else(
                || "Unsaved document".to_string(),
                |path| {
                    self.root
                        .as_deref()
                        .filter(|root| path.starts_with(root))
                        .map(|root| mt_core::workspace::display_relative(root, path))
                        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
                },
            );
            let status = i18n::t(
                document_details_status_key(document.is_externally_changed(), document.is_dirty()),
                cx,
            )
            .to_string();
            (
                document.title(cx),
                document.document().doc_type().label().to_string(),
                location,
                status,
            )
        };
        let accessibility_label = format!("{}: {title}", i18n::t(i18n::Key::Details, cx));

        Some(
            v_flex()
                .id("document-details")
                .role(gpui_kit::Role::DescriptionList)
                .aria_label(accessibility_label)
                .p(metrics::inset())
                .gap(metrics::gap())
                .child(div().text_sm().font_semibold().child(title))
                .child(detail_field(
                    "document-detail-kind",
                    cx,
                    i18n::t(i18n::Key::Kind, cx),
                    kind,
                ))
                .child(detail_field(
                    "document-detail-location",
                    cx,
                    i18n::t(i18n::Key::Location, cx),
                    location,
                ))
                .child(detail_field(
                    "document-detail-status",
                    cx,
                    i18n::t(i18n::Key::Status, cx),
                    status,
                ))
                .into_any_element(),
        )
    }

    /// Contextual details for the active document or selected Harness artifact.
    fn render_right_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.right_panel_open {
            return None;
        }
        let active_document_id = self
            .active_document()
            .map(|document| document.read(cx).id());
        let review_belongs_to_active_document = active_document_id.is_some_and(|document_id| {
            self.review_flow.review_result_belongs_to(document_id)
                || self.review_flow.review_diagnostic_belongs_to(document_id)
        });
        let review_target_belongs_to_active_document = active_document_id
            .and_then(|document_id| self.review_flow.review_target_for_document(document_id))
            .is_some();
        let pending_review_belongs_to_active_document = active_document_id
            .is_some_and(|document_id| self.review_flow.pending_review_belongs_to(document_id));
        if self.review_flow.review_panel_is_open()
            && ((self.review_flow.is_reviewing() && pending_review_belongs_to_active_document)
                || review_target_belongs_to_active_document
                || review_belongs_to_active_document)
        {
            let dismiss_review = Button::new("dismiss-review")
                .icon(IconName::Close)
                .xsmall()
                .ghost()
                .tooltip(i18n::t(i18n::Key::ReviewDismiss, cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dismiss_review(cx);
                }));
            let review_panel = if self.review_flow.review_result().is_none()
                && !self.review_flow.is_reviewing()
                && !self.review_flow.has_revision_context()
                && self.review_flow.review_diagnostic().is_none()
                && self
                    .recovered_revision_answers_for_active_document(cx)
                    .is_some()
            {
                self.render_recovered_revision_panel(cx)
            } else if self.review_flow.review_result().is_none()
                && !self.review_flow.is_reviewing()
                && !self.review_flow.has_revision_context()
                && self.review_flow.review_diagnostic().is_none()
            {
                self.render_review_idle_panel(cx)
            } else {
                self.render_review_panel(cx)
            };
            return Some(
                v_flex()
                    .size_full()
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .h(metrics::row())
                            .flex_shrink_0()
                            .px(metrics::inset())
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(i18n::t(i18n::Key::Review, cx)),
                            )
                            .child(dismiss_review),
                    )
                    .child(review_panel)
                    .into_any_element(),
            );
        }
        let harness_selected = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.read(cx).has_selection());
        let details = match details_content(
            self.side_panel,
            self.settings_open,
            self.active_document().is_some(),
            harness_selected,
        ) {
            DetailsContent::Harness => self
                .harness
                .clone()
                .map(|harness| harness.update(cx, |harness, cx| harness.render_details(cx)))
                .unwrap_or_else(|| div().into_any_element()),
            DetailsContent::Document => self
                .render_document_details(cx)
                .unwrap_or_else(|| div().into_any_element()),
            DetailsContent::Empty => div().into_any_element(),
        };
        Some(
            v_flex()
                .size_full()
                .bg(cx.theme().sidebar)
                .child(
                    h_flex()
                        .h(metrics::row())
                        .flex_shrink_0()
                        .px(metrics::inset())
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground)
                                .child(i18n::t(i18n::Key::Details, cx)),
                        ),
                )
                .child(
                    div()
                        .id("details")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(details),
                )
                .into_any_element(),
        )
    }

    /// The left panel: persistent vertical navigation above the selected tool.
    fn render_side_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .py_2()
                    .children(SidePanel::ALL.map(|panel| {
                        SidebarNavigationButton::new(
                            panel.id(),
                            panel.icon(),
                            i18n::t(panel.label(), cx),
                        )
                        .selected(panel == self.side_panel)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.side_panel = panel;
                            cx.notify();
                        }))
                    })),
            )
            .child(div().flex_1().min_h_0().map(|this| match self.side_panel {
                SidePanel::Files => match &self.explorer {
                    Some(explorer) => this.child(explorer.clone()),
                    None => this.child(empty_hint(cx, i18n::t(i18n::Key::OpenFolderToBegin, cx))),
                },
                // No folder needed: "this file" and "open tabs" work the
                // moment a document is open, so gating the whole panel on a
                // workspace would hide two of its four scopes for no reason.
                SidePanel::Search => this.child(self.search.clone()),
                SidePanel::Harness => match &self.harness {
                    Some(harness) => this.child(harness.clone()),
                    None => {
                        this.child(empty_hint(cx, i18n::t(i18n::Key::OpenFolderToDiscover, cx)))
                    }
                },
                SidePanel::Outline => this.child(self.render_outline(cx)),
            }))
    }

    /// Document outline: headings plus MDX structure.
    ///
    /// Every row navigates: an outline you cannot click is a table of contents
    /// with the page numbers torn off.
    fn render_outline(&self, cx: &Context<Self>) -> AnyElement {
        let Some(doc) = self.active_document() else {
            return empty_hint(cx, i18n::t(i18n::Key::OpenDocumentForOutline, cx))
                .into_any_element();
        };
        let doc = doc.read(cx);
        let outline = doc.document().outline();
        if outline.is_empty() {
            return empty_hint(cx, i18n::t(i18n::Key::NoHeadings, cx)).into_any_element();
        }

        v_flex()
            .id("outline")
            .size_full()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .overflow_y_scroll()
            .children(outline.headings.iter().enumerate().map(|(ix, h)| {
                let offset = h.offset;
                ListItem::new(("outline-heading", ix))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .child(
                        div()
                            // Indentation carries the heading level, so the
                            // padding has to be on the content rather than the
                            // row — otherwise the hover highlight steps in with
                            // it and the list looks ragged.
                            .pl(metrics::indent(h.depth.saturating_sub(1) as usize))
                            .text_sm()
                            .truncate()
                            .child(h.text.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_offset(offset, window, cx)
                    }))
            }))
            .children(outline.structural.iter().enumerate().map(|(ix, entry)| {
                let offset = entry.offset;
                ListItem::new(("outline-structural", ix))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(entry.kind.label())
                            .child(div().flex_1().truncate().child(entry.label.clone())),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_offset(offset, window, cx)
                    }))
            }))
            .into_any_element()
    }

    /// Move the active document's cursor to `offset` and show it.
    fn reveal_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        // Path-only history cannot navigate a recovered buffer by its source
        // path without selecting a different, ordinary file tab.
        if let Some(path) = self
            .tabs
            .active()
            .and_then(|tab| tab.path())
            .map(Path::to_path_buf)
        {
            self.record_visit(path, offset);
        }
        doc.update(cx, |doc, cx| doc.reveal_offset(offset, window, cx));
    }

    /// Open a file-targeted search result as a preview at its source offset.
    fn reveal_in(
        &mut self,
        path: PathBuf,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open_file_as(path.clone(), true, window, cx) {
            return;
        }
        let Some(tab) = self.tabs.active() else {
            return;
        };
        if !matches!(&tab.identity, TabIdentity::File(current) if current == &path) {
            return;
        }
        let document = tab.payload.view.clone();
        if document.read(cx).source_path() != Some(path.as_path()) {
            return;
        }
        self.record_visit(path, offset);
        document.update(cx, |document, cx| {
            document.reveal_offset(offset, window, cx)
        });
    }

    /// Search hits in recovered-only tabs identify the open buffer, not its
    /// source path. A closed result is inert; reopening that path would reveal
    /// a different document's text. Path-only history cannot represent this
    /// tab without redirecting Back to the ordinary file.
    fn reveal_open_document(
        &mut self,
        id: DocumentId,
        source_path: &Path,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.document_index(id, cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        if !matches!(&tab.identity, TabIdentity::Recovered(_))
            || tab.payload.view.read(cx).source_path() != Some(source_path)
        {
            return;
        }
        if !self.tabs.focus(ix) {
            return;
        }
        let document = self
            .document_at(ix)
            .cloned()
            .expect("a focused document index must exist");
        document.update(cx, |document, cx| {
            document.reveal_offset(offset, window, cx)
        });
        self.web_dirty(cx);
        cx.notify();
    }

    /// Show the Search panel and put the caret in its field.
    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.side_panel = SidePanel::Search;
        // Opening the panel is not enough: the point of the binding is to type
        // a query immediately, and a panel that appears without focus makes the
        // user click into it first.
        if !self.left_panel_open {
            self.left_panel_open = true;
        }
        // Cloned first: leasing the entity through `update` takes its own
        // borrow of `cx`, which cannot overlap one held through `self`.
        let search = self.search.clone();
        search.update(cx, |search, cx| search.focus_query(window, cx));
        self.web_dirty(cx);
        cx.notify();
    }

    /// The documents the current search scope covers.
    ///
    /// Built here rather than in the search view because only the workspace can
    /// see all four sources — and because the open tabs must contribute their
    /// *editor* text, not the file on disk, or a search misses everything the
    /// user has typed since the last save.
    ///
    /// Directories are handed over unwalked. This runs on the UI thread, and
    /// walking a real vault is seconds — measured at 2.4s for 6,642 documents
    /// — so expanding a root here would freeze the window on every settled
    /// keystroke. [`Corpus::roots`] is walked on the search's own task.
    fn search_corpus(&self, cx: &App) -> Corpus {
        use crate::views::search::Scope;

        let mut corpus = Corpus::default();
        let add_open = |corpus: &mut Corpus, identity: &TabIdentity, doc: &DocumentView| {
            let (path, target) = match identity {
                TabIdentity::File(path) => (path.as_path(), SearchTarget::File),
                TabIdentity::Recovered(_) => {
                    let Some(path) = doc.source_path() else {
                        return;
                    };
                    (path, SearchTarget::OpenDocument(doc.id()))
                }
                TabIdentity::Memory(_) => return,
            };
            corpus.open.push(OpenSnapshot {
                path: path.to_path_buf(),
                text: doc.text(cx),
                target,
            });
        };
        let add_open_tabs = |corpus: &mut Corpus| {
            for tab in self.tabs.iter() {
                let doc = tab.payload.view.read(cx);
                add_open(corpus, &tab.identity, doc);
            }
        };

        match self.search.read(cx).scope() {
            Scope::Document => {
                if let Some(tab) = self.tabs.active() {
                    let doc = tab.payload.view.read(cx);
                    add_open(&mut corpus, &tab.identity, doc);
                }
            }
            Scope::OpenTabs => add_open_tabs(&mut corpus),
            Scope::Folder => {
                add_open_tabs(&mut corpus);
                corpus.roots.extend(self.root.clone());
            }
            Scope::Harness => {
                add_open_tabs(&mut corpus);
                // The whole point of this scope: a skill's own directory holds
                // references and scripts beside its SKILL.md, and those are as
                // much a part of the skill as its entry document.
                if let Some(harness) = &self.harness {
                    let harness = harness.read(cx);
                    corpus
                        .roots
                        .extend(harness.skills().iter().map(|s| s.dir.clone()));
                    corpus
                        .files
                        .extend(harness.instructions().iter().map(|i| i.path.clone()));
                }
            }
        }
        // The open tabs are already in `corpus.open` with their unsaved text;
        // reading them again would report every match twice.
        let open = self.open_paths(cx);
        corpus.files.retain(|p| !open.contains(p));
        corpus
    }

    /// Paths of every open tab.
    fn open_paths(&self, cx: &App) -> Vec<PathBuf> {
        let _ = cx;
        self.tabs.paths().map(Path::to_path_buf).collect()
    }

    /// The left panel's toggle.
    ///
    /// Its fixed home is the global title bar, so opening the panel never moves
    /// the control the user needs to close it again.
    fn render_left_toggle(&self, tooltip: bool, cx: &Context<Self>) -> impl IntoElement {
        ChromeIconButton::new(
            "toggle-left-panel",
            IconName::PanelLeft,
            i18n::t(i18n::Key::ToggleLeftPanel, cx),
        )
        .pressed(self.left_panel_open)
        .when(tooltip, |button| {
            button.tooltip(i18n::t(i18n::Key::ToggleLeftPanel, cx))
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.on_toggle_left_panel(&ToggleLeftPanel, window, cx)
        }))
    }

    /// The right panel's fixed toggle in the global title bar.
    fn render_right_toggle(&self, tooltip: bool, cx: &Context<Self>) -> impl IntoElement {
        ChromeIconButton::new(
            "toggle-right-panel",
            IconName::PanelRight,
            i18n::t(i18n::Key::ToggleRightPanel, cx),
        )
        .pressed(self.right_panel_open)
        .when(tooltip, |button| {
            button.tooltip(i18n::t(i18n::Key::ToggleRightPanel, cx))
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.on_toggle_right_panel(&ToggleRightPanel, window, cx)
        }))
    }

    fn render_tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let root = self.root.clone();
        let web_active = self.web_active(cx);
        TabBar::new("document-tabs")
            // Not `w_full`: the bar sits inside the title bar, and a strip that
            // claims the whole width leaves no slack for dragging the window.
            // Shrink-to-fit means the tabs take what they need and the rest of
            // the bar stays a drag handle.
            .large()
            .selected_index(self.tabs.active_index())
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let doc = tab.payload.view.read(cx);
                let recovered_only = matches!(&tab.identity, TabIdentity::Recovered(_));
                let tab_path = tab.path();
                // Recovered identities stay pathless in `Tabs`; only file
                // affordances may consult their payload's original source.
                let recovered_path = recovered_only.then(|| doc.source_path()).flatten();
                let action_path = tab_path.or(recovered_path);
                let document_title = doc.title(cx);
                let recovered_prefix = i18n::t(i18n::Key::RecoveredSnapshot, cx);
                let recovered_index = ix + 1;
                let full = if recovered_only {
                    let location = recovered_path
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| document_title.clone());
                    format!("{recovered_index} {recovered_prefix}: {location}")
                } else {
                    tab_path
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| "Unsaved document".to_string())
                };
                // Relative only makes sense with a folder open, and only for a
                // file actually under it — a globally-discovered skill is not.
                let relative = action_path
                    .zip(root.as_deref())
                    .and_then(|(path, root)| path.strip_prefix(root).ok())
                    .map(|rest| rest.to_string_lossy().replace('\\', "/"));
                let is_preview = tab_path.is_some_and(|path| self.tabs.is_preview(path));
                let dirty = doc.is_dirty();
                let active_single_document = self.tabs.len() == 1 && self.tabs.active_index() == ix;
                let label = if recovered_only {
                    elide_tab_label(&format!(
                        "{recovered_index} {recovered_prefix}: {document_title}"
                    ))
                } else {
                    elide_tab_label(&document_title)
                };
                let aria_label = if dirty {
                    format!("{label}, {}", i18n::t(i18n::Key::UnsavedChanges, cx))
                } else {
                    label.clone()
                };

                Tab::new()
                    .label(label)
                    .aria_label(aria_label)
                    .when(doc.is_externally_changed(), |tab| {
                        tab.icon(IconName::TriangleAlert)
                    })
                    .when(!web_active && action_path.is_some(), |tab| {
                        tab.child(
                            div()
                                .id(SharedString::from(format!("tab-affordances-{ix}")))
                                .absolute()
                                .inset_0()
                                .tooltip({
                                    let full = full.clone();
                                    move |window, cx| Tooltip::new(full.clone()).build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Right, {
                                    cx.listener(move |this, _, _, cx| {
                                        this.tabs.set_menu(ix);
                                        cx.notify();
                                    })
                                })
                                .context_menu({
                                    let relative = relative.clone();
                                    move |menu, _window, cx| {
                                        let menu = menu.menu(
                                            i18n::t(i18n::Key::CopyPath, cx),
                                            Box::new(CopyPath),
                                        );
                                        match relative {
                                            Some(_) => menu.menu(
                                                i18n::t(i18n::Key::CopyRelativePath, cx),
                                                Box::new(CopyRelativePath),
                                            ),
                                            None => menu,
                                        }
                                    }
                                }),
                        )
                    })
                    // A preview tab is italic, the same signal VS Code uses, so
                    // "this will be replaced by the next click" is visible
                    // before it happens rather than after.
                    .when(is_preview, |tab| tab.italic())
                    .suffix(
                        // Unsaved work shows a dot where the close button
                        // would be, which is the convention every editor uses
                        // and the reason the marker is here rather than
                        // appended to the label: a long name elides, and the
                        // one tab that most needs the warning was the one
                        // losing it.
                        if dirty {
                            div()
                                .id(SharedString::from(format!("dirty-{ix}")))
                                .role(gpui_kit::Role::Button)
                                .aria_label("Close document")
                                .when(active_single_document, |this| {
                                    this.accessibility_id(TAB_CLOSE_ACCESSIBILITY_ID)
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(metrics::target())
                                .when(!web_active, |this| {
                                    this.tooltip(move |window, cx| {
                                        Tooltip::new(i18n::t(i18n::Key::UnsavedChanges, cx))
                                            .build(window, cx)
                                    })
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.request_close_tab(ix, window, cx);
                                }))
                                .child(
                                    div()
                                        .size(metrics::dirty_dot())
                                        .rounded_full()
                                        .bg(cx.theme().primary),
                                )
                                .into_any_element()
                        } else {
                            Button::new(SharedString::from(format!("close-{ix}")))
                                .icon(IconName::Close)
                                .accessibility_label("Close document")
                                .when(active_single_document, |button| {
                                    button.accessibility_id(TAB_CLOSE_ACCESSIBILITY_ID)
                                })
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.request_close_tab(ix, window, cx);
                                }))
                                .into_any_element()
                        },
                    )
            }))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if this.tabs.focus(*ix)
                    && let Some(tab) = this.tabs.get(*ix)
                    && let Some(path) = tab.path()
                {
                    this.record_visit(path.to_path_buf(), 0);
                }
                this.web_dirty(cx);
                cx.notify();
            }))
    }

    fn render_web_path_controls(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.web_active(cx) {
            return None;
        }
        let path = self
            .active_document()?
            .read(cx)
            .source_path()
            .map(Path::to_path_buf)?;
        let has_relative = self
            .root
            .as_ref()
            .is_some_and(|root| path.strip_prefix(root).is_ok());

        Some(
            h_flex()
                .id("web-path-commands")
                .flex_shrink_0()
                .min_w_0()
                .gap_0p5()
                .items_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("web-copy-path")
                        .icon(IconName::Copy)
                        .label(i18n::t(i18n::Key::CopyPath, cx))
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.tabs.clear_menu();
                            this.on_copy_path(&CopyPath, window, cx);
                        })),
                )
                .when(has_relative, |this| {
                    this.child(
                        Button::new("web-copy-relative-path")
                            .icon(IconName::Copy)
                            .label(i18n::t(i18n::Key::CopyRelativePath, cx))
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.tabs.clear_menu();
                                this.on_copy_relative_path(&CopyRelativePath, window, cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_panel_resize_handle(
        &self,
        edge: WorkspaceResizeEdge,
        geometry: WorkspaceResizeGeometry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match edge {
            WorkspaceResizeEdge::Left => "left-panel-resize-handle",
            WorkspaceResizeEdge::Right => "right-panel-resize-handle",
        };
        let group = match edge {
            WorkspaceResizeEdge::Left => "left-panel-resize-group",
            WorkspaceResizeEdge::Right => "right-panel-resize-group",
        };
        let label = i18n::t(
            match edge {
                WorkspaceResizeEdge::Left => i18n::Key::SidePanelWidth,
                WorkspaceResizeEdge::Right => i18n::Key::DetailsPanelWidth,
            },
            cx,
        );
        let focus_handle = window
            .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_index(0)
            .tab_stop(true);
        let focus_on_press = focus_handle.clone();
        let is_focused = focus_handle.is_focused(window);
        let increment = cx.entity().downgrade();
        let decrement = increment.clone();
        let set_value = increment.clone();
        let line = cx.theme().border;
        let hover = cx.theme().primary;
        // accesskit_consumer 0.37 exposes a Splitter's RangeValue provider as
        // read-only on Windows even when SetValue is handled. Keep the same
        // interaction contract under Slider until the consumer is fixed.
        let (role, orientation) = if cfg!(target_os = "windows") {
            (
                gpui_kit::Role::Slider,
                gpui_kit::accesskit::Orientation::Horizontal,
            )
        } else {
            (
                gpui_kit::Role::Splitter,
                gpui_kit::accesskit::Orientation::Vertical,
            )
        };

        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(role)
            .aria_label(label)
            .aria_orientation(orientation)
            .aria_numeric_value(f32::from(geometry.width) as f64)
            .aria_min_numeric_value(f32::from(geometry.minimum) as f64)
            .aria_max_numeric_value(f32::from(geometry.maximum) as f64)
            .aria_numeric_value_step(f32::from(metrics::gap_group()) as f64)
            .track_focus(&focus_handle)
            .group(group)
            .occlude()
            .absolute()
            .top_0()
            .left(geometry.boundary - px(4.))
            .h_full()
            .w(px(9.))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    focus_on_press.focus(window, cx);
                    this.panel_resize_grab = Some(WorkspaceResizeGrab {
                        edge,
                        pointer_offset: event.position.x - geometry.boundary,
                    });
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                this.on_panel_resize_key_down(edge, event, window, cx)
            }))
            .on_a11y_action(
                gpui_kit::accesskit::Action::Increment,
                move |_, window, cx| {
                    if let Some(this) = increment.upgrade() {
                        this.update(cx, |this, cx| {
                            this.resize_panel_by_delta(edge, metrics::gap_group(), window, cx);
                        });
                    }
                },
            )
            .on_a11y_action(
                gpui_kit::accesskit::Action::Decrement,
                move |_, window, cx| {
                    if let Some(this) = decrement.upgrade() {
                        this.update(cx, |this, cx| {
                            this.resize_panel_by_delta(edge, -metrics::gap_group(), window, cx);
                        });
                    }
                },
            )
            .on_a11y_action(
                gpui_kit::accesskit::Action::SetValue,
                move |data, window, cx| {
                    let Some(gpui_kit::accesskit::ActionData::NumericValue(value)) = data else {
                        return;
                    };
                    if !value.is_finite() {
                        return;
                    }
                    if let Some(this) = set_value.upgrade() {
                        this.update(cx, |this, cx| {
                            this.set_panel_width(edge, px(*value as f32), window, cx);
                        });
                    }
                },
            )
            .on_drag(edge, |_, _, _, cx| cx.new(|_| WorkspaceResizePreview))
            .on_drag_move::<WorkspaceResizeEdge>(cx.listener(Self::on_panel_resize_drag))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.panel_resize_grab = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.panel_resize_grab = None),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(4.))
                    .h_full()
                    .w(px(1.))
                    .bg(if is_focused { hover } else { line })
                    .group_hover(group, |this| this.bg(hover)),
            )
            .into_any_element()
    }

    fn render_title_commands(&self, tooltips: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap(metrics::gap())
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!self.show_welcome, |this| {
                this.child(
                    ChromeIconButton::new(
                        "open-folder",
                        IconName::FolderOpen,
                        i18n::t(i18n::Key::OpenFolderPicker, cx),
                    )
                    .when(tooltips, |button| {
                        button.tooltip(i18n::t(i18n::Key::OpenFolderPicker, cx))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_open_folder(&OpenFolder, window, cx)
                    })),
                )
                .child(
                    ChromeIconButton::new(
                        "translate",
                        IconName::Globe,
                        i18n::t(i18n::Key::Translate, cx),
                    )
                    .loading(self.translating)
                    .when(tooltips, |button| {
                        button.tooltip(i18n::t(i18n::Key::Translate, cx))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        let has_selection = this
                            .active_document()
                            .is_some_and(|document| !document.read(cx).selection(cx).is_empty());
                        if has_selection {
                            this.on_translate_selection(&TranslateSelection, window, cx)
                        } else {
                            this.on_translate_document(&TranslateDocument, window, cx)
                        }
                    })),
                )
                .child(
                    ChromeIconButton::new(
                        "review",
                        IconName::Search,
                        i18n::t(i18n::Key::Review, cx),
                    )
                    .loading(self.review_flow.is_reviewing())
                    .when(tooltips, |button| {
                        button.tooltip(i18n::t(i18n::Key::Review, cx))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        let has_selection = this
                            .active_document()
                            .is_some_and(|document| !document.read(cx).selection(cx).is_empty());
                        if has_selection {
                            this.on_review_selection(&ReviewSelection, window, cx)
                        } else {
                            this.on_review_document(&ReviewDocument, window, cx)
                        }
                    })),
                )
                .child(self.render_right_toggle(tooltips, cx))
            })
            .child(
                ChromeIconButton::new(
                    "settings",
                    IconName::Settings,
                    i18n::t(i18n::Key::Settings, cx),
                )
                .pressed(self.settings_open)
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::Settings, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_open_settings(&OpenSettings, window, cx)
                })),
            )
    }

    fn render_left_toggle_overlay(&self, tooltips: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .id("workspace-left-toggle")
            .absolute()
            .top_0()
            .left_0()
            .h(metrics::title_bar())
            .items_center()
            .pl(metrics::title_bar_leading_inset())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!self.show_welcome, |this| {
                this.child(self.render_left_toggle(tooltips, cx))
            })
    }

    fn render_title_commands_overlay(
        &self,
        tooltips: bool,
        native_controls_width: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("workspace-title-commands")
            .absolute()
            .top_0()
            .right(native_controls_width)
            .h(metrics::title_bar())
            .w(metrics::title_commands())
            .items_center()
            .justify_end()
            .px(metrics::gap())
            .child(self.render_title_commands(tooltips, cx))
    }

    /// Platform title-bar behavior and native controls, underneath the
    /// application-owned workspace columns.
    fn render_title_bar_backdrop(&self, cx: &Context<Self>) -> impl IntoElement {
        let workspace = cx.entity().downgrade();
        TitleBar::new()
            .on_close_window(move |_, window, cx| {
                let should_close = workspace
                    .update(cx, |workspace, cx| {
                        workspace.request_window_close(window, cx)
                    })
                    .unwrap_or(true);
                if should_close {
                    Self::post_native_window_close(window);
                }
            })
            .h(metrics::title_bar())
            .border_b_0()
            .bg(cx.theme().title_bar)
    }

    fn render_left_title_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .h(metrics::title_bar())
            .w_full()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
    }

    fn render_document_title_bar(
        &self,
        native_controls_width: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .h(metrics::title_bar())
            .w_full()
            .flex_none()
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().title_bar),
            )
            .when(native_controls_width > px(0.), |this| {
                this.child(
                    div()
                        .h_full()
                        .w(native_controls_width)
                        .flex_none()
                        .border_b_1()
                        .border_color(cx.theme().border),
                )
            })
    }

    /// The title controls are one stable semantic row above the workspace
    /// body. Their horizontal track uses the same owned panel widths as the
    /// background and body rows, so focus identity and AccessKit order do not
    /// depend on which panels are open.
    fn render_document_title_controls(
        &self,
        left: Pixels,
        right: Pixels,
        reserve_left_toggle: bool,
        reserve_commands: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let web_active = self.web_active(cx);
        let navigator = self.render_navigator(!web_active, cx).into_any_element();
        let web_path_controls = self.render_web_path_controls(cx);
        h_flex()
            .absolute()
            .top_0()
            .left(left)
            .right(right)
            .h(metrics::title_bar())
            .min_w_0()
            .items_center()
            .gap(metrics::gap())
            .when(reserve_left_toggle, |this| {
                this.pl(metrics::title_bar_leading_inset() + metrics::target() + metrics::gap())
            })
            .when(!reserve_left_toggle, |this| this.pl(metrics::gap()))
            // Back and Forward first, then the tabs — the arrangement Zed and
            // every browser use, because navigation is about the strip that
            // follows it.
            .child(navigator)
            // The tabs claim the press; the slack beside them does not. A
            // handler on the flex filler would cover the title bar's drag area.
            .when(!self.tabs.is_empty(), |this| {
                this.child(
                    div()
                        .min_w_0()
                        .max_w_full()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.render_tabs(cx)),
                )
            })
            .when(self.tabs.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .child("markturbo"),
                )
            })
            .child(div().flex_1().min_w_0().h_full())
            .when_some(web_path_controls, |this, controls| this.child(controls))
            .when(reserve_commands, |this| {
                this.child(div().h_full().w(metrics::title_commands()).flex_none())
            })
            .when(!reserve_commands, |this| this.pr(metrics::gap()))
    }

    fn render_right_title_bar(
        &self,
        native_controls_width: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .h(metrics::title_bar())
            .w_full()
            .flex_none()
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar),
            )
            .when(native_controls_width > px(0.), |this| {
                this.child(
                    div()
                        .h_full()
                        .w(native_controls_width)
                        .flex_none()
                        .border_b_1()
                        .border_color(cx.theme().border),
                )
            })
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let renderers = self
            .registry
            .availability_report()
            .into_iter()
            .filter(|(_, a)| !a.is_available())
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        let watching = crate::settings::AppSettings::global(cx).watch_auto_reload;
        let web_active = self.web_active(cx);
        let auto_refresh_label = i18n::t(
            if watching {
                i18n::Key::AutoRefreshOn
            } else {
                i18n::Key::AutoRefresh
            },
            cx,
        );
        let status = self
            .recovery_warning()
            .map(str::to_owned)
            .or_else(|| self.status.clone());

        h_flex()
            .w_full()
            .h(metrics::status_bar())
            .px(metrics::inset())
            .gap(metrics::gap_group())
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().status_bar)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .children(status.clone().map(|s| div().flex_1().child(s)))
            .when(status.is_none(), |this| {
                this.child(
                    div().flex_1().children(
                        self.root
                            .as_ref()
                            .map(|root| div().child(root.to_string_lossy().to_string())),
                    ),
                )
            })
            .when(!renderers.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(format!("{} unavailable", renderers.join(", "))),
                )
            })
            // Last, so it lands at the right end of the bar. Watching is a mode
            // rather than a command, and a mode needs somewhere to show that it
            // is on — an eye that is lit is the whole indicator, so the same
            // control both sets and reports it.
            .child(
                Button::new("toggle-auto-refresh")
                    .icon(if watching {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .xsmall()
                    .ghost()
                    .when(watching, |b| b.primary())
                    // A tooltip would open across the child HWND and be covered.
                    // Web mode therefore names the command in fixed chrome.
                    .when(web_active, |button| button.label(auto_refresh_label))
                    .when(!web_active, |button| button.tooltip(auto_refresh_label))
                    .on_click(cx.listener(|_, _, _, cx| {
                        crate::settings::AppSettings::update(cx, |settings| {
                            settings.watch_auto_reload = !settings.watch_auto_reload
                        });
                    })),
            )
    }
}

fn empty_hint(cx: &App, text: &str) -> impl IntoElement {
    div()
        .p(metrics::inset())
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

fn detail_field(
    id: &'static str,
    cx: &App,
    name: &str,
    value: impl Into<SharedString>,
) -> AnyElement {
    let value = value.into();
    h_flex()
        .id(id)
        .role(gpui_kit::Role::Group)
        .aria_label(format!("{name}: {value}"))
        .gap_2()
        .items_start()
        .text_xs()
        .child(
            div()
                .w(metrics::details_label())
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(name.to_string()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The WebView is deliberately NOT touched here: it is an OS child
        // window, and mutating it during a draw re-enters the window procedure
        // with the App already borrowed. `WebSurface::mark_dirty` runs it after
        // the effect cycle instead.

        // Preferences are initialized from the first viewport, then remain
        // stable through maximize/restore. This pass only clamps them when the
        // current window is too narrow to preserve the document column.
        let viewport = self.layout_width.unwrap_or(window.viewport_size().width);
        let content: AnyElement = if self.settings_open {
            self.settings.clone().into_any_element()
        } else if self.show_welcome {
            self.render_welcome(cx)
        } else {
            match self.active_document() {
                Some(doc) => doc.clone().into_any_element(),
                None => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Icon::new(IconName::BookOpen))
                    .child(
                        div()
                            .text_sm()
                            .child(i18n::t(i18n::Key::OpenAMarkdownFile, cx).to_string()),
                    )
                    .into_any_element(),
            }
        };

        // All regions are built before the tree, because each takes a borrow of `cx`:
        // the details panel leases the harness entity, and the title bar and
        // side panel read the active document through it. Building them inline
        // would overlap those borrows with the `&mut Context` the element chain
        // already holds.
        let left_panel_visible = !self.show_welcome && self.left_panel_open;
        let right_panel = (!self.show_welcome)
            .then(|| self.render_right_panel(cx))
            .flatten();
        let right_panel_visible = right_panel.is_some();
        let side_panel = left_panel_visible.then(|| self.render_side_panel(cx).into_any_element());
        let panel_widths = resolved_workspace_panel_widths(
            self.preferred_left_panel_width,
            self.preferred_right_panel_width,
            left_panel_visible,
            right_panel_visible,
            viewport,
        );
        let web_active = self.web_active(cx);
        let controls_width = native_window_controls_width(window);
        let left_toggle_overlay = self
            .render_left_toggle_overlay(!web_active, cx)
            .into_any_element();
        let title_commands_overlay = self
            .render_title_commands_overlay(!web_active, controls_width, cx)
            .into_any_element();
        let document_controls_right = if right_panel_visible {
            panel_widths.right
        } else {
            controls_width
        };
        let document_title_controls = self
            .render_document_title_controls(
                panel_widths.left,
                document_controls_right,
                !left_panel_visible,
                !right_panel_visible,
                cx,
            )
            .into_any_element();
        let left_title_region = left_panel_visible.then(|| {
            workspace_region(
                "left-title-region",
                Some(panel_widths.left),
                self.render_left_title_bar(cx).into_any_element(),
            )
        });
        let document_title_region = workspace_region(
            "document-title-region",
            None,
            self.render_document_title_bar(
                if right_panel_visible {
                    px(0.)
                } else {
                    controls_width
                },
                cx,
            )
            .into_any_element(),
        );
        let right_title_region = right_panel_visible.then(|| {
            workspace_region(
                "right-title-region",
                Some(panel_widths.right),
                self.render_right_title_bar(controls_width, cx)
                    .into_any_element(),
            )
        });
        let left_column = side_panel
            .map(|panel| workspace_region("left-workspace-column", Some(panel_widths.left), panel));
        let left_resize_handle = left_column.as_ref().map(|_| {
            self.render_panel_resize_handle(
                WorkspaceResizeEdge::Left,
                workspace_resize_geometry(
                    WorkspaceResizeEdge::Left,
                    panel_widths,
                    left_panel_visible,
                    right_panel_visible,
                    viewport,
                ),
                window,
                cx,
            )
        });
        let document_column = workspace_region("document-workspace-column", None, content);
        let right_column = right_panel.map(|panel| {
            workspace_region("right-workspace-column", Some(panel_widths.right), panel)
        });
        let right_resize_handle = right_column.as_ref().map(|_| {
            self.render_panel_resize_handle(
                WorkspaceResizeEdge::Right,
                workspace_resize_geometry(
                    WorkspaceResizeEdge::Right,
                    panel_widths,
                    self.left_panel_open,
                    right_panel_visible,
                    viewport,
                ),
                window,
                cx,
            )
        });
        let title_bar_backdrop = self.render_title_bar_backdrop(cx).into_any_element();
        let title_regions = h_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when_some(left_title_region, |this, region| this.child(region))
            .child(document_title_region)
            .when_some(right_title_region, |this, region| this.child(region));
        let workspace_title = div()
            .relative()
            .w_full()
            .h(metrics::title_bar())
            .flex_none()
            .min_w_0()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(metrics::title_bar())
                    .child(title_bar_backdrop),
            )
            .child(title_regions)
            .child(left_toggle_overlay)
            .child(document_title_controls)
            .child(title_commands_overlay);
        let body_regions = h_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when_some(left_column, |this, column| this.child(column))
            .child(document_column)
            .when_some(right_column, |this, column| this.child(column));
        let workspace_body = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(body_regions)
            .when_some(left_resize_handle, |this, handle| this.child(handle))
            .when_some(right_resize_handle, |this, handle| this.child(handle));
        let workspace_frame = v_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            // Title controls precede body content in the actual element tree,
            // so paint, Tab traversal, and AccessKit browse order all describe
            // the same interface. Both rows receive the same owned widths.
            .child(workspace_title)
            .child(workspace_body);

        let this = cx.entity().downgrade();
        v_flex()
            .id("workspace")
            // Without a role the whole window is announced instead of the
            // focused element; gpui logs exactly that. `Application` is the
            // right one for a window whose own keybindings drive it.
            .role(gpui_kit::Role::Application)
            .aria_label("markturbo workspace")
            .track_focus(&self.focus_handle)
            .key_context(if self.show_welcome && !self.settings_open {
                welcome::WELCOME_KEY_CONTEXT
            } else {
                "Workspace"
            })
            .on_action(cx.listener(Self::on_new_document))
            .on_action(cx.listener(Self::on_paste_into_new))
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_copy_path))
            .on_action(cx.listener(Self::on_copy_relative_path))
            .on_action(cx.listener(Self::on_toggle_left_panel))
            .on_action(cx.listener(Self::on_toggle_right_panel))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_navigate_back))
            .on_action(cx.listener(Self::on_navigate_forward))
            // Dropping a file or folder onto the window opens it. The whole
            // window is the target rather than the document area: a drop is
            // aimed at the app, and making the user find a hot zone would be a
            // puzzle rather than a feature.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.on_drop_paths(paths.paths(), window, cx);
            }))
            .on_action(cx.listener(Self::on_translate_document))
            .on_action(cx.listener(Self::on_translate_selection))
            .on_action(cx.listener(Self::on_translate_block))
            .on_action(cx.listener(Self::on_review_document))
            .on_action(cx.listener(Self::on_review_selection))
            .on_action(cx.listener(Self::on_cancel_review))
            .on_action(|_: &AcknowledgeStartupInput, _, _| {
                crate::startup::record(StartupEvent::FirstInputHandled);
            })
            .on_prepaint(move |bounds, _, cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| {
                        let width = bounds.size.width;
                        if this.layout_width != Some(width) {
                            this.layout_width = Some(width);
                            cx.notify();
                        }
                    });
                }
            })
            .size_full()
            .child(workspace_frame)
            .child(self.render_status_bar(cx))
    }
}

/// Keybindings for the workspace's actions.
pub fn init(cx: &mut App) {
    crate::credentials::AppCredentialVault::init(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewDocument, None),
        KeyBinding::new("ctrl-n", NewDocument, None),
        KeyBinding::new("cmd-v", PasteIntoNew, Some(welcome::WELCOME_KEY_CONTEXT)),
        KeyBinding::new("ctrl-v", PasteIntoNew, Some(welcome::WELCOME_KEY_CONTEXT)),
        KeyBinding::new("cmd-o", OpenFile, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("cmd-shift-o", OpenFolder, None),
        KeyBinding::new("ctrl-alt-o", OpenFolder, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("cmd-shift-s", SaveAs, None),
        KeyBinding::new("ctrl-shift-s", SaveAs, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        // The platform convention for preferences on each host.
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        // All three translation scopes are reachable: document, the editor
        // selection, and the block under the cursor.
        KeyBinding::new("cmd-shift-t", TranslateDocument, None),
        KeyBinding::new("ctrl-shift-t", TranslateDocument, None),
        KeyBinding::new("cmd-shift-l", TranslateSelection, None),
        KeyBinding::new("ctrl-shift-l", TranslateSelection, None),
        KeyBinding::new("cmd-shift-b", TranslateBlock, None),
        KeyBinding::new("ctrl-shift-b", TranslateBlock, None),
        KeyBinding::new("cmd-shift-r", ReviewDocument, None),
        KeyBinding::new("ctrl-shift-r", ReviewDocument, None),
        KeyBinding::new("cmd-shift-alt-r", ReviewSelection, None),
        KeyBinding::new("ctrl-shift-alt-r", ReviewSelection, None),
        // The panels, on the bindings VS Code uses for the same two.
        KeyBinding::new("cmd-b", ToggleLeftPanel, None),
        KeyBinding::new("ctrl-b", ToggleLeftPanel, None),
        KeyBinding::new("cmd-alt-b", ToggleRightPanel, None),
        KeyBinding::new("ctrl-alt-b", ToggleRightPanel, None),
        // Workspace search. `Ctrl/Cmd+F` stays with the editor's own find —
        // one searches the document you are in, the other searches everywhere,
        // and giving the second the first's binding would take away the more
        // frequently wanted of the two.
        KeyBinding::new("cmd-shift-f", FocusSearch, None),
        KeyBinding::new("ctrl-shift-f", FocusSearch, None),
        // Back and forward, on the bindings every editor and IDE uses.
        KeyBinding::new("ctrl-alt--", NavigateBack, None),
        KeyBinding::new("ctrl-alt-shift--", NavigateForward, None),
        KeyBinding::new("cmd-alt-left", NavigateBack, None),
        KeyBinding::new("cmd-alt-right", NavigateForward, None),
        KeyBinding::new("alt-left", NavigateBack, None),
        KeyBinding::new("alt-right", NavigateForward, None),
    ]);
}

#[cfg(test)]
mod tests {
    mod effective_context;
    mod revision_context;
    mod welcome;

    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        fs,
        path::Path,
        path::PathBuf,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    // Import selectively: the `gpui_kit::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{
        DestructiveAction, DestructiveRequest, DestructiveResolution, DetailsContent,
        DirtyDecision, DocumentRecoveryState, DocumentView, RecoveryAttempt,
        RecoveryContentIdentity, ReviewRequestBuildError, ReviewTarget, SaveAsMode, SaveAsOutcome,
        SaveMode, SearchEvent, SearchTarget, SidePanel, StartupRecovery, TAB_LABEL_MAX, Workspace,
        WorkspaceResizeEdge, checkpoint_batch_status, clamped_dragged_panel_width,
        current_checkpoint_write_completed_for_identity, details_content,
        document_details_status_key, elide_tab_label, hex_bytes, next_review_lens,
        path_affects_harness, prepare_recovery_records, resolved_workspace_panel_widths,
        revision_answer_is_incorporated, revision_apply_identity_matches, startup_recovery_status,
    };
    use crate::i18n;
    use crate::views::Layout;
    use crate::web::{self, Trust};
    use gpui_kit::{
        AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Modifiers, MouseButton,
        TestAppContext, VisualTestContext, Window, point, px,
    };
    use mt_core::document::io::{FileStamp, Newline, SourceIdentity};
    use mt_core::model::RevisionRequestBinding;
    use mt_core::recovery::{
        CheckpointBatchOutcome, CheckpointOutcome, CheckpointSchedule, RecoveredRecord,
        RecoveryCheckpoint, RecoveryError, RecoveryIssue, RecoveryKey, RecoveryLimits,
        RecoveryMaintenance, RecoveryMetadata, RecoveryProtector, RecoveryRecord, RecoveryScan,
        RecoveryStore, RecoveryToken, RetirementCompletion, RevisionRecovery,
    };
    use mt_core::review::provider::{
        RevisionAnswer, RevisionAnswers, RevisionQuestionCoverageStatus, decode_revision_capture,
        revision_question_id,
    };
    use mt_core::review::revision::ChangeId;
    use mt_core::review::{
        ArtifactLens, ClarificationPriority, ClarificationQuestion, ReviewDiagnosticCode,
        ReviewModelOutput, ReviewSections, SourceAnchor, SourceLocation, SourceSnapshot,
    };
    use mt_core::workspace::search::{Query, Results, search_files, search_open_document};
    use mt_core::workspace::watcher::Change;

    fn build_document_review_request(
        target: ReviewTarget,
        lens: ArtifactLens,
        source_path: Option<&Path>,
        full_text: &str,
        selection: Option<&std::ops::Range<usize>>,
        snapshot: SourceSnapshot,
    ) -> Result<mt_core::review::ReviewRequest, ReviewRequestBuildError> {
        super::build_review_request(super::ReviewRequestBuildRequest::new(
            target,
            lens,
            source_path,
            full_text,
            selection,
            snapshot,
            false,
        ))
        .map(|built| built.into_parts().0)
    }

    fn install_frozen_skill_review(
        workspace: &Entity<Workspace>,
        relative_path: &str,
        location: SourceLocation,
        cx: &mut VisualTestContext,
    ) -> SourceAnchor {
        let anchor = SourceAnchor::agent_skill_file(relative_path, location).unwrap();
        workspace.update(cx, |workspace, cx| {
            let document = workspace.active_document().unwrap().clone();
            let (document_id, source_path, text, is_dirty, source_snapshot, origin, source) = {
                let document = document.read(cx);
                let source_snapshot = document.async_snapshot(cx);
                let source =
                    SourceSnapshot::new(document.revision(), source_snapshot.source_generation());
                (
                    document.id(),
                    document.source_path().map(Path::to_path_buf),
                    document.text(cx),
                    document.is_dirty(),
                    source_snapshot,
                    document.skill_origin().cloned(),
                    source,
                )
            };
            let origin = origin.expect("Skill Review fixture must use an actual loaded origin");
            let (request, skill_package) = super::build_review_request(
                super::ReviewRequestBuildRequest::new(
                    ReviewTarget::Document,
                    ArtifactLens::AgentSkill,
                    source_path.as_deref(),
                    &text,
                    None,
                    source,
                    is_dirty,
                )
                .with_skill_origin(Some(&origin)),
            )
            .unwrap()
            .into_parts();
            let skill_package = skill_package.expect("Skill Review freezes its inventory");
            let output = ReviewModelOutput {
                schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
                scope: request.scope,
                understood_intent: ReviewSections {
                    stated_goal: "navigate the frozen source".into(),
                    relevant_context: Vec::new(),
                    constraints: Vec::new(),
                    non_goals: Vec::new(),
                    expected_deliverable: "a source anchor".into(),
                    success_evidence: Vec::new(),
                    inferred_assumptions: Vec::new(),
                    unresolved_decisions: Vec::new(),
                },
                findings: vec![mt_core::review::Finding::inference(
                    "Inspect this source location",
                    anchor.clone(),
                )],
                clarification_questions: Vec::new(),
            };
            let result = mt_core::review::ReviewResult::ready(&request, output)
                .expect("test anchor belongs to the frozen package");
            let partial = request
                .source
                .package()
                .is_some_and(mt_core::review::SkillPackage::is_partial);
            workspace.review_flow.install_review_result(
                super::WorkspaceReviewResult {
                    document_id,
                    source_snapshot,
                    target: ReviewTarget::Document,
                    selection: None,
                    lens: ArtifactLens::AgentSkill,
                    partial,
                    skill_package: Some(skill_package),
                    supporting_sources_current: true,
                    result: mt_core::review::provider::ReviewTransportResult {
                        result,
                        metadata: mt_core::review::provider::ReviewMetadata::from_response(
                            mt_core::model::Provider::OpenAiResponses,
                            "test-model",
                            "test-model",
                        )
                        .unwrap(),
                    },
                },
                false,
            );
        });
        anchor
    }

    fn open_test_workspace(
        cx: &mut TestAppContext,
        initial: PathBuf,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        open_test_workspace_with(cx, Some(initial))
    }

    fn open_test_workspace_with(
        cx: &mut TestAppContext,
        initial: Option<PathBuf>,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        let recovery_root = tempfile::tempdir().unwrap();
        let recovery = RecoveryStore::new_at(
            recovery_root.path().join("store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace_with_recovery_store(cx, initial, recovery);
        workspace.update(cx, |workspace, _| {
            workspace._test_recovery_root = Some(recovery_root);
        });
        (workspace, cx)
    }

    fn open_test_workspace_with_recovery_store(
        cx: &mut TestAppContext,
        initial: Option<PathBuf>,
        recovery: RecoveryStore,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        // Preserve the settled empty-startup path; store-owning tests inject
        // their store once that arbitration has completed.
        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, initial, StartupRecovery::default);
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.startup_recovery_pending = false;
            workspace.recovery_flow.recovery = Some(recovery);
        });
        (workspace, cx)
    }

    fn open_test_workspace_with_startup_recovery<F>(
        cx: &mut TestAppContext,
        initial: Option<PathBuf>,
        load_startup_recovery: F,
    ) -> (Entity<Workspace>, &mut VisualTestContext)
    where
        F: FnOnce() -> StartupRecovery + Send + 'static,
    {
        open_test_workspace_with_startup_recovery_inspection(
            cx,
            initial,
            load_startup_recovery,
            |_, _, _| {},
        )
    }

    fn open_test_workspace_with_startup_recovery_inspection<F, G>(
        cx: &mut TestAppContext,
        initial: Option<PathBuf>,
        load_startup_recovery: F,
        inspect_before_startup: G,
    ) -> (Entity<Workspace>, &mut VisualTestContext)
    where
        F: FnOnce() -> StartupRecovery + Send + 'static,
        G: FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    {
        let initial_is_none = initial.is_none();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::settings::AppSettings::init(cx);
            // Most workspace tests use an empty state as a fixture for
            // document and panel behavior. First-use tests opt in below.
            crate::settings::AppSettings::update(cx, |settings| {
                settings.show_welcome_on_startup = false;
            });
            super::init(cx);
        });
        let captured = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::new_with_startup_recovery(
                        initial,
                        load_startup_recovery,
                        window,
                        cx,
                    );
                    inspect_before_startup(&mut workspace, window, cx);
                    workspace
                });
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_kit::component::Root::new(workspace, window, cx)
            }
        });
        let workspace = captured.borrow().clone().expect("the Workspace entity");
        if initial_is_none {
            workspace.update(cx, |workspace, _| {
                // This helper is the legacy empty-workspace fixture. Product
                // startup behavior is covered by the explicit welcome helpers.
                let _ = workspace.tabs.close(0);
            });
        }
        cx.update(|window, app| {
            let handle = workspace.read(app).focus_handle(app);
            window.focus(&handle, app);
            window.draw(app).clear(app);
        });
        cx.update(|window, app| window.draw(app).clear(app));
        (workspace, cx)
    }

    #[cfg(feature = "model-transport")]
    fn one_shot_translation_server() -> (String, std::sync::mpsc::Receiver<()>, Arc<AtomicUsize>) {
        use std::io::{BufRead as _, BufReader, Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = requests.clone();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            request_count.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            sender.send(()).unwrap();

            let response =
                r#"{"choices":[{"message":{"role":"assistant","content":"[\"Bonjour\"]"}}]}"#;
            let mut stream = &stream;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
            stream.flush().unwrap();
        });

        (format!("http://{address}/v1/"), receiver, requests)
    }

    #[cfg(feature = "model-transport")]
    fn configure_test_translation(cx: &mut VisualTestContext, base_url: &str) {
        use mt_core::model::{EndpointIdentity, Provider};

        let endpoint = EndpointIdentity::parse(Provider::OpenAiChat, Some(base_url)).unwrap();
        cx.update(|_, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.model_provider = Provider::OpenAiChat.key().into();
                settings.model_name = "translation-test-model".into();
                settings.model_base_url = base_url.into();
                settings.translate_to = "fr".into();
            });
            crate::credentials::AppCredentialVault::global(app)
                .replace_session(
                    endpoint.credential_target().to_string(),
                    "synthetic-workspace-credential".into(),
                )
                .unwrap();
        });
    }

    fn open_test_workspace_with_welcome_preference(
        cx: &mut TestAppContext,
        show_welcome_on_startup: bool,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::settings::AppSettings::init(cx);
            crate::settings::AppSettings::update(cx, |settings| {
                settings.show_welcome_on_startup = show_welcome_on_startup;
            });
            super::init(cx);
        });
        let captured = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| {
                    Workspace::new_with_startup_recovery(None, StartupRecovery::default, window, cx)
                });
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_kit::component::Root::new(workspace, window, cx)
            }
        });
        let workspace = captured.borrow().clone().expect("the Workspace entity");
        cx.update(|window, app| {
            let handle = workspace.read(app).focus_handle(app);
            window.focus(&handle, app);
            window.draw(app).clear(app);
        });
        (workspace, cx)
    }

    fn populated_startup_recovery(store: RecoveryStore) -> StartupRecovery {
        let scan = store.recover().unwrap();
        let scan_issues = scan.issues.len();
        let (documents, preparation_issues) = prepare_recovery_records(scan.records);
        StartupRecovery {
            recovery: Some(store),
            documents,
            recovery_issue_count: scan_issues + preparation_issues,
            recovery_error: None,
        }
    }

    fn write_recovery_checkpoint(store: &RecoveryStore, path: &Path, text: &str) {
        let loaded = mt_core::document::io::load(path).unwrap();
        let checkpoint = RecoveryCheckpoint {
            key: RecoveryKey::for_path(path),
            text: text.to_string(),
            metadata: RecoveryMetadata::from_loaded_file(&loaded),
            revision: None,
        };
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();
    }

    fn emit_search_reveal(
        workspace: &Entity<Workspace>,
        event: SearchEvent,
        cx: &mut VisualTestContext,
    ) {
        let search = workspace.read_with(cx, |workspace, _| workspace.search.clone());
        search.update(cx, |_, cx| cx.emit(event));
    }

    fn restore_open_file_checkpoint(
        workspace: &Entity<Workspace>,
        path: &Path,
        store: &RecoveryStore,
        cx: &mut VisualTestContext,
    ) -> (mt_core::document::lifecycle::DocumentId, usize) {
        let live = workspace
            .read_with(cx, |workspace, _| {
                workspace.tabs.iter().find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(open_path)
                        if open_path.as_path() == path =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
            })
            .expect("the ordinary source tab");
        live.update(cx, |document, _| document.rotate_recovery_key());

        let scan = store.recover().unwrap();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert_eq!(
                    restore_recovery_for_test(workspace, scan, window, cx),
                    (1, 0)
                );
            });
        });
        cx.run_until_parked();

        let key = RecoveryKey::for_path(path);
        let (id, index) = workspace.read_with(cx, |workspace, app| {
            workspace
                .tabs
                .iter()
                .enumerate()
                .find_map(|(index, tab)| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::Recovered(recovered_key)
                        if recovered_key == &key =>
                    {
                        Some((tab.payload.view.read(app).id(), index))
                    }
                    _ => None,
                })
                .expect("the checkpoint is restored as a recovered sibling")
        });
        workspace.update(cx, |workspace, _| assert!(workspace.tabs.focus(index)));
        (id, index)
    }

    fn write_memory_recovery_checkpoint(store: &RecoveryStore, text: &str) -> RecoveryKey {
        let key = RecoveryKey::new_memory();
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key: key.clone(),
                    text: text.to_string(),
                    metadata: RecoveryMetadata {
                        source_path: None,
                        encoding_name: "UTF-8".to_string(),
                        had_bom: false,
                        newline: Newline::Lf,
                        original_stamp: FileStamp {
                            modified: None,
                            len: 0,
                            digest: [0; 32],
                            object_id: None,
                        },
                        source_identity: SourceIdentity::Regular,
                        decode_had_errors: false,
                    },
                    revision: None,
                },
                &HashSet::new(),
            )
            .unwrap();
        key
    }

    fn restore_recovery_for_test(
        workspace: &mut Workspace,
        scan: RecoveryScan,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> (usize, usize) {
        let scan_issues = scan.issues.len();
        let (documents, preparation_issues) = prepare_recovery_records(scan.records);
        let (restored, restore_skipped) =
            workspace.restore_prepared_recovery(documents, None, window, cx);
        (restored, scan_issues + preparation_issues + restore_skipped)
    }

    fn restore_startup_recovery_for_test(
        workspace: &mut Workspace,
        scan: RecoveryScan,
        recovery_issue_count: usize,
        recovery_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let (documents, preparation_issues) = prepare_recovery_records(scan.records);
        let startup_targets = workspace.startup_recovery_targets(cx);
        workspace.restore_startup_recovery(
            StartupRecovery {
                recovery: workspace.recovery_flow.recovery.clone(),
                documents,
                recovery_issue_count: recovery_issue_count + preparation_issues,
                recovery_error,
            },
            startup_targets,
            window,
            cx,
        );
    }

    fn complete_startup_with_store(
        workspace: &Entity<Workspace>,
        store: RecoveryStore,
        cx: &mut VisualTestContext,
    ) {
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(
                    StartupRecovery {
                        recovery: Some(store),
                        ..StartupRecovery::default()
                    },
                    startup_targets,
                    window,
                    cx,
                );
            });
        });
    }

    fn replace_document(
        workspace: &Entity<Workspace>,
        ix: usize,
        text: &str,
        cx: &mut VisualTestContext,
    ) {
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(ix).cloned())
            .expect("the document tab");
        cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text(text.to_string(), window, cx);
            });
        });
        cx.run_until_parked();
    }

    fn document_text(workspace: &Entity<Workspace>, ix: usize, cx: &VisualTestContext) -> String {
        workspace.read_with(cx, |workspace, app| {
            workspace
                .document_at(ix)
                .expect("the document tab")
                .read(app)
                .text(app)
        })
    }

    fn assert_failed_save_preserves_document(
        workspace: &Entity<Workspace>,
        text: &str,
        externally_changed: bool,
        status: &str,
        cx: &VisualTestContext,
    ) {
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .expect("the document tab");
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert_eq!(document.is_externally_changed(), externally_changed);
            assert_eq!(document.text(app), text);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.status.as_deref(), Some(status));
        });
    }

    struct TestRecoveryProtector;

    impl RecoveryProtector for TestRecoveryProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    enum CountingProtection {
        Reversible,
        Expand(usize),
        FailOnce,
    }

    struct CountingRecoveryProtector {
        calls: AtomicUsize,
        behavior: CountingProtection,
    }

    impl CountingRecoveryProtector {
        fn new(behavior: CountingProtection) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                behavior,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl RecoveryProtector for CountingRecoveryProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if matches!(self.behavior, CountingProtection::FailOnce) && call == 0 {
                return Err(RecoveryError::Protection);
            }
            let mut ciphertext: Vec<_> = plaintext.iter().rev().copied().collect();
            if let CountingProtection::Expand(bytes) = self.behavior {
                ciphertext.resize(ciphertext.len() + bytes, 0);
            }
            Ok(ciphertext)
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    fn recovery_limits(max_record_bytes: u64) -> RecoveryLimits {
        RecoveryLimits {
            max_record_bytes,
            ..RecoveryLimits::default()
        }
    }

    fn test_recovery_attempt(
        token: RecoveryToken,
        revision: u64,
        now: Instant,
        cancelled: Arc<AtomicBool>,
    ) -> RecoveryAttempt {
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let timing = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        RecoveryAttempt {
            token,
            content_identity: RecoveryContentIdentity::for_revision(revision),
            timing,
            cancelled,
        }
    }

    #[gpui_kit::test]
    fn a_clean_tab_closes_immediately(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clean.md");
        fs::write(&path, "clean\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);

        cx.simulate_keystrokes("ctrl-w");

        assert!(!cx.has_pending_prompt());
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
    }

    #[gpui_kit::test]
    fn memory_documents_are_pathless_dirty_when_pasted_and_excluded_from_path_only_surfaces(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with(cx, None);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory("# Pasted prompt\nbody\n".to_string(), window, cx);
            });
        });

        let document = workspace
            .read_with(cx, |workspace, app| {
                assert!(matches!(
                    workspace.tabs.active().map(|tab| &tab.identity),
                    Some(super::TabIdentity::Memory(_))
                ));
                assert!(workspace.tabs.active().and_then(|tab| tab.path()).is_none());
                assert!(workspace.search_corpus(app).open.is_empty());
                workspace.document_at(0).cloned()
            })
            .expect("the memory document");
        document.read_with(cx, |document, app| {
            assert_eq!(document.source_path(), None);
            assert_eq!(document.title(app), "Pasted prompt");
            assert!(document.is_dirty());
            assert_eq!(document.layout(), Layout::Source);
            assert!(!document.watches_path(Path::new("C:/not-a-document.md")));
            assert_eq!(document.recovery_checkpoint(app).metadata.source_path, None);
        });

        cx.update(|_window, app| {
            document.update(app, |document, cx| document.set_layout(Layout::Web, cx));
            workspace.update(app, |workspace, cx| {
                assert!(workspace.render_web_path_controls(cx).is_none());
            });
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(String::new(), window, cx);
            });
        });
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(1).unwrap().read(app);
            assert_eq!(document.source_path(), None);
            assert_eq!(document.title(app), "Untitled");
            assert!(!document.is_dirty());
        });
    }

    #[gpui_kit::test]
    fn save_as_migrates_a_memory_document_to_a_file_identity(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved-from-memory.md");
        let text = "# Saved prompt\nexact text\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let source_generation = Rc::new(RefCell::new(None));
        cx.update(|window, app| {
            let source_generation = source_generation.clone();
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.to_string(), window, cx);
                let id = workspace.document_at(0).unwrap().read(cx).id();
                *source_generation.borrow_mut() = Some(
                    workspace
                        .document_at(0)
                        .unwrap()
                        .read(cx)
                        .async_snapshot(cx)
                        .source_generation(),
                );
                workspace.finish_save_as(id, path.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        workspace.read_with(cx, |workspace, app| {
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::File(candidate)) if candidate == &path
            ));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(path.as_path()));
            assert!(!document.is_dirty());
            assert_eq!(
                document.async_snapshot(app).source_generation(),
                source_generation
                    .borrow()
                    .expect("the initial source generation")
                    + 1
            );
            assert_eq!(workspace.root.as_deref(), path.parent());
            assert_eq!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .first()
                    .map(|target| target.path.as_path()),
                Some(path.as_path())
            );
        });

        let edited = "# Saved prompt\nsubsequent save \u{4fdd}\u{7559} \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_save(&super::Save, window, cx);
            });
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);

        fs::write(&path, "external version\n").unwrap();
        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.apply_watcher_changes(dir.path(), &[Change::Modified(path.clone())], cx);
            });
        });
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(path.as_path()));
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), edited);
        });
    }

    #[gpui_kit::test]
    fn save_as_outside_the_workspace_keeps_real_watcher_conflict_detection(
        cx: &mut TestAppContext,
    ) {
        let workspace_dir = tempfile::tempdir().unwrap();
        let external_dir = tempfile::tempdir().unwrap();
        let original = workspace_dir.path().join("original.md");
        let destination = external_dir.path().join("saved-as.md");
        let text = "editor text stays visible\n";
        fs::write(&original, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, original);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        std::thread::sleep(Duration::from_millis(200));
        fs::write(&destination, "external rewrite\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            workspace.update(cx, |workspace, cx| workspace.drain_watcher(cx));
            let conflicted = workspace.read_with(cx, |workspace, app| {
                workspace
                    .document_at(0)
                    .unwrap()
                    .read(app)
                    .is_externally_changed()
            });
            if conflicted {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(destination.as_path()));
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), text);
        });
    }

    #[gpui_kit::test]
    fn clean_window_close_defers_teardown_until_the_focused_input_handler_can_drain(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.dont_show_welcome_again(window, cx);
            });
        });

        cx.update(|window, app| {
            assert!(window.focused(app).is_some());
            let should_close = workspace.update(app, |workspace, cx| {
                workspace.request_window_close(window, cx)
            });
            assert!(!should_close);
            assert!(window.focused(app).is_none());
            assert!(workspace.read(app).window_close_pending);
            assert!(!workspace.read(app).window_close_ready);

            workspace.update(app, |workspace, cx| {
                workspace.window_close_ready = true;
                assert!(workspace.request_window_close(window, cx));
            });
        });
    }

    #[gpui_kit::test]
    fn cancelling_save_as_overwrite_keeps_destination_and_buffer_byte_identical(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("existing.md");
        let original_bytes = b"external bytes\xFF\n";
        fs::write(&destination, original_bytes).unwrap();
        let text = "editor text\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "an existing Save As destination must require a separate Replace decision"
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(fs::read(&destination).unwrap(), original_bytes);
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::Memory(_))
            ));
            assert!(workspace.pending_destructive.is_none());
        });
    }

    #[gpui_kit::test]
    fn confirmed_save_as_overwrite_replaces_only_the_selected_destination(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("existing.md");
        let untouched = dir.path().join("untouched.md");
        fs::write(&destination, "external version\n").unwrap();
        fs::write(&untouched, "do not replace\n").unwrap();
        let text = "editor text \u{4fdd}\u{7559} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&destination).unwrap(), text);
        assert_eq!(fs::read_to_string(&untouched).unwrap(), "do not replace\n");
        workspace.read_with(cx, |workspace, app| {
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::File(path)) if path == &destination
            ));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(destination.as_path()));
            assert!(!document.is_dirty());
            assert_eq!(document.text(app), text);
        });
    }

    #[gpui_kit::test]
    fn replace_confirmation_refuses_a_destination_changed_while_the_prompt_is_open(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("existing.md");
        fs::write(&destination, "version shown in the prompt\n").unwrap();
        let text = "editor text\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        assert!(cx.has_pending_prompt());
        fs::write(&destination, "later external version\n").unwrap();
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();

        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "later external version\n"
        );
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.pending_destructive.is_none());
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert_eq!(document.source_path(), None);
            assert_eq!(
                workspace.status.as_deref(),
                Some("Save As failed: the file changed on disk since it was opened; reload or save a copy")
            );
        });
    }

    #[gpui_kit::test]
    fn save_as_picker_cancellation_is_a_total_no_op(cx: &mut TestAppContext) {
        let text = "unsaved \u{4fdd}\u{7559} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as_selection(id, None, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.pending_destructive.is_none());
            assert!(workspace.status.is_none());
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::Memory(_))
            ));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
        });
    }

    #[gpui_kit::test]
    fn save_as_rejects_an_equivalent_path_already_open_in_another_tab(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let open_path = dir.path().join("open.md");
        let equivalent_path = dir.path().join(".").join("open.md");
        fs::write(&open_path, "open document\n").unwrap();
        let text = "second editor\n";
        let (workspace, cx) = open_test_workspace(cx, open_path.clone());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                let id = workspace.document_at(1).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    equivalent_path.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });

        assert_eq!(fs::read_to_string(&open_path).unwrap(), "open document\n");
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let expected = i18n::save_as_path_already_open_message(&equivalent_path, app);
            assert_eq!(workspace.status.as_deref(), Some(expected.as_str()));
            let document = workspace.document_at(1).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::Memory(_))
            ));
        });
    }

    #[gpui_kit::test]
    fn memory_dirty_close_save_keeps_the_destructive_request_open_for_save_as(
        cx: &mut TestAppContext,
    ) {
        let text = "CJK \u{4fdd}\u{7559} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                workspace.document_at(0).unwrap().read(cx).id()
            })
        });

        cx.simulate_keystrokes("ctrl-w");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(
                workspace
                    .pending_destructive
                    .as_ref()
                    .and_then(DestructiveRequest::current),
                Some(id),
                "Save As must retain the exact destructive request until its write succeeds"
            );
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
        });

        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("saved-after-close.md");
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(destination).unwrap(), text);
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
    }

    #[gpui_kit::test]
    fn cancelling_memory_dirty_close_save_as_keeps_the_buffer_open_and_recoverable(
        cx: &mut TestAppContext,
    ) {
        let text = "CJK \u{4fdd}\u{7559} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                workspace.document_at(0).unwrap().read(cx).id()
            })
        });

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as_selection(id, None, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.pending_destructive.is_none());
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert!(workspace.recovery_flow.recovery_schedules.contains_key(&id));
        });
    }

    #[gpui_kit::test]
    fn cancelling_an_existing_save_as_destination_keeps_a_dirty_close_buffer_open(
        cx: &mut TestAppContext,
    ) {
        let text = "CJK close save \u{4fdd}\u{7559} \u{1f680}\n";
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("existing.md");
        let original = b"destination remains unchanged\n";
        fs::write(&destination, original).unwrap();
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                workspace.document_at(0).unwrap().read(cx).id()
            })
        });

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        assert_eq!(fs::read(&destination).unwrap(), original);
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.pending_destructive.is_none());
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.recovery_flow.recovery_schedules.contains_key(&id));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert_eq!(document.source_path(), None);
        });
    }

    #[gpui_kit::test]
    fn replacing_an_existing_save_as_destination_completes_the_dirty_close(
        cx: &mut TestAppContext,
    ) {
        let text = "CJK close replace \u{4fdd}\u{7559} \u{1f680}\n";
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("existing.md");
        fs::write(&destination, "old destination\n").unwrap();
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(text.into(), window, cx);
                workspace.document_at(0).unwrap().read(cx).id()
            })
        });

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&destination).unwrap(), text);
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.pending_destructive.is_none());
            assert!(workspace.tabs.is_empty());
            assert!(!workspace.recovery_flow.recovery_schedules.contains_key(&id));
            assert_eq!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .first()
                    .map(|target| target.path.as_path()),
                Some(destination.as_path())
            );
        });
    }

    #[gpui_kit::test]
    fn dropping_a_folder_then_file_uses_the_shared_target_lifecycle(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let document_path = folder.path().join("dropped.md");
        let text = "# Dropped \u{4fdd}\u{7559} \u{1f680}\n";
        fs::write(&document_path, text).unwrap();
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_drop_paths(&[folder.path().to_path_buf()], window, cx);
            });
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert_eq!(workspace.root.as_deref(), Some(folder.path()));
            assert!(workspace.tabs.is_empty());
            let recent = crate::settings::AppSettings::global(app)
                .recent_targets
                .first()
                .expect("dropped folder is recent");
            assert_eq!(recent.path.as_path(), folder.path());
            assert_eq!(recent.kind, mt_core::settings::RecentTargetKind::Workspace);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_drop_paths(std::slice::from_ref(&document_path), window, cx);
            });
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.root.as_deref(), Some(folder.path()));
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(document_path.as_path()));
            assert_eq!(document.text(app), text);
            assert!(!document.is_dirty());
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_schedules
                    .contains_key(&document.id()),
                "a clean dropped file must not enter dirty-buffer recovery"
            );
            let recents = &crate::settings::AppSettings::global(app).recent_targets;
            assert_eq!(recents[0].path, document_path);
            assert_eq!(recents[0].kind, mt_core::settings::RecentTargetKind::File);
            assert_eq!(recents[1].path, folder.path());
            assert_eq!(
                recents[1].kind,
                mt_core::settings::RecentTargetKind::Workspace
            );
        });
    }

    #[gpui_kit::test]
    fn save_as_snapshot_drift_cancels_the_pending_close_without_writing(cx: &mut TestAppContext) {
        let initial = "initial \u{4fdd}\u{7559} \u{1f680}\n";
        let revised = "revised \u{4fdd}\u{7559} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory(initial.into(), window, cx);
                workspace.document_at(0).unwrap().read(cx).id()
            })
        });

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        replace_document(&workspace, 0, revised, cx);

        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("must-not-write.md");
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });

        assert!(!destination.exists());
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.pending_destructive.is_none());
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.recovery_flow.recovery_schedules.contains_key(&id));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), revised);
            assert!(document.is_dirty());
        });
    }

    #[gpui_kit::test]
    fn pathless_recovery_restores_as_a_memory_document_with_its_original_key(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let key = write_memory_recovery_checkpoint(&store, "# Recovered prompt\n");
        let scan = store.recover().unwrap();
        let (workspace, cx) = open_test_workspace_with_recovery_store(cx, None, store);
        let restored = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_recovery_for_test(workspace, scan, window, cx)
            })
        });
        assert_eq!(restored, (1, 0));
        workspace.read_with(cx, |workspace, app| {
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::Memory(_))
            ));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), None);
            assert_eq!(document.recovery_key(), key);
            assert!(document.is_dirty());
            assert_eq!(document.text(app), "# Recovered prompt\n");
        });
    }

    #[gpui_kit::test]
    fn failed_history_navigation_keeps_the_active_memory_document_and_no_preview(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.md");
        let second = dir.path().join("second.md");
        fs::write(&second, "second\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, second.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory("unsaved\n".to_string(), window, cx);
                workspace.record_visit(missing.clone(), 0);
                workspace.record_visit(second, 0);
            });
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_navigate_back(&super::NavigateBack, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert!(matches!(
                workspace.tabs.active().map(|tab| &tab.identity),
                Some(super::TabIdentity::Memory(_))
            ));
            assert!(workspace.tabs.preview().is_none());
            assert_eq!(
                workspace
                    .document_at(workspace.tabs.active_index())
                    .unwrap()
                    .read(app)
                    .text(app),
                "unsaved\n"
            );
        });
    }

    #[gpui_kit::test]
    fn dirty_close_saves_exact_text_before_closing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.md");
        fs::write(&path, "before\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "中文 draft \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.simulate_keystrokes("ctrl-w");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
        assert_eq!(fs::read_to_string(path).unwrap(), edited);
    }

    #[gpui_kit::test]
    fn dirty_close_discard_never_writes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        replace_document(&workspace, 0, "editor only\n", cx);

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
    }

    #[gpui_kit::test]
    fn discard_waits_for_startup_store_before_closing_and_retiring(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-before-startup.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "checkpoint before discard\n");
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });
        replace_document(&workspace, 0, "discarded editor text\n", cx);
        let id = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).id()
        });
        let key = RecoveryKey::for_path(&path);

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.pending_startup_destructive.is_some());
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Waiting for recovery storage to clear its checkpoint. The document remains open."
                )
            );
        });
        assert_eq!(store.recover().unwrap().records.len(), 1);

        complete_startup_with_store(&workspace, store.clone(), cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .contains_key(&key)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
            assert!(
                !workspace.recovery_flow.recovery_schedules.contains_key(&id),
                "startup repair must not re-arm a document being durably discarded"
            );
        });
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
        assert!(store.recover().unwrap().records.is_empty());
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
    }

    #[gpui_kit::test]
    fn cancelling_a_new_dirty_prompt_rearms_a_startup_discarded_document(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("startup-discard-first.md");
        let second = dir.path().join("startup-discard-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
                workspace.recovery_flow.recovery = None;
                workspace.recovery_flow.startup_recovery_pending = true;
            });
        });

        let latest_first = "first text kept after cancelled close\n";
        replace_document(&workspace, 0, latest_first, cx);
        let (first_id, first_checkpoint) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (document.id(), document.recovery_checkpoint(app))
        });
        let first_key = first_checkpoint.key.clone();
        store
            .checkpoint(&first_checkpoint, &HashSet::from([first_key.clone()]))
            .unwrap();

        assert!(!cx.simulate_close());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        complete_startup_with_store(&workspace, store.clone(), cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .contains_key(&first_key)
            );
        });

        let second_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(1).cloned())
            .unwrap();
        cx.update(|window, app| {
            second_document.update(app, |document, cx| {
                document.replace_text("second became dirty\n".into(), window, cx);
            });
        });
        cx.run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "the newly dirty second document must be decided before window close"
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .pending_destructive_recovery
                    .iter()
                    .any(|(key, _)| key == &first_key)
            );
        });
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert_eq!(document_text(&workspace, 0, cx), latest_first);
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.document_at(0).unwrap().read(app).is_dirty());
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&first_id)
                .expect("the still-dirty first document must be re-armed after revalidation");
            assert!(state.token.is_some());
            assert!(state.schedule.next_deadline().is_some());
        });

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|record| (record.record.key, record.record.text))
            .collect();
        assert_eq!(
            recovered.get(&first_key).map(String::as_str),
            Some(latest_first)
        );
    }

    #[gpui_kit::test]
    fn save_waits_for_startup_store_before_closing_and_retiring(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save-before-startup.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "checkpoint before save\n");
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });
        let edited = "saved while recovery starts\n";
        replace_document(&workspace, 0, edited, cx);

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.pending_startup_destructive.is_some());
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        assert_eq!(store.recover().unwrap().records.len(), 1);

        complete_startup_with_store(&workspace, store.clone(), cx);
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            0
        );
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn startup_close_retires_live_aliases_but_preserves_old_checkpoints(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.md");
        let second = dir.path().join("second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &first, "stale first checkpoint\n");
        write_recovery_checkpoint(&store, &second, "stale second checkpoint\n");
        let first_key = RecoveryKey::for_path(&first);
        let second_key = RecoveryKey::for_path(&second);
        assert_eq!(store.recover().unwrap().records.len(), 2);

        let (workspace, cx) = open_test_workspace(cx, first.clone());
        let first_id = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).id()
        });
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
            workspace
                .recovery_flow
                .startup_recovery_keys
                .insert(first_id, first_key.clone());
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });
        let second_id = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(1).unwrap().read(app).id()
        });

        let saved_second = "saved second document\n";
        replace_document(&workspace, 1, saved_second, cx);
        let second_live_key = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(1).unwrap().read(app).recovery_key()
        });
        assert_ne!(second_live_key, second_key);
        let second_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(1).cloned())
            .unwrap();
        second_document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        cx.run_until_parked();
        assert_eq!(fs::read_to_string(&second).unwrap(), saved_second);

        replace_document(&workspace, 0, "discarded first document\n", cx);
        let first_live_key = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).recovery_key()
        });
        assert_ne!(first_live_key, first_key);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_close_tab(0, window, cx);
            });
        });
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.pending_startup_destructive.is_some());
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&first_live_key)
            );
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&second_live_key)
            );
            assert_eq!(
                workspace.recovery_flow.startup_recovery_keys.get(&first_id),
                Some(&first_key)
            );
            assert_eq!(
                workspace
                    .recovery_flow
                    .startup_recovery_keys
                    .get(&second_id),
                Some(&second_key)
            );
        });
        assert_eq!(store.recover().unwrap().records.len(), 2);

        let startup = populated_startup_recovery(store.clone());
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, startup_targets, window, cx);
            });
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 3);
            assert!(workspace.tabs.index_of(&first).is_none());
            assert!(workspace.tabs.index_of(&second).is_some());
            let live_second = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(path) if path == &second => {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the saved live B tab must remain open");
            let document = live_second.read(app);
            assert!(!document.is_dirty());
            assert_eq!(document.text(app), saved_second);

            for (key, expected_text) in [
                (&first_key, "stale first checkpoint\n"),
                (&second_key, "stale second checkpoint\n"),
            ] {
                let recovered = workspace
                    .tabs
                    .iter()
                    .find_map(|tab| match &tab.identity {
                        mt_core::workspace::tabs::TabIdentity::Recovered(recovered_key)
                            if recovered_key == key =>
                        {
                            Some(tab.payload.view.clone())
                        }
                        _ => None,
                    })
                    .expect("each older checkpoint must remain separately visible");
                let recovered = recovered.read(app);
                assert_eq!(recovered.text(app), expected_text);
                assert!(recovered.is_dirty());
            }
        });
        assert_eq!(fs::read_to_string(&first).unwrap(), "first disk\n");
        assert_eq!(fs::read_to_string(&second).unwrap(), saved_second);
        let recovered = store.recover().unwrap().records;
        assert_eq!(recovered.len(), 2);
        assert_eq!(
            recovered
                .iter()
                .find(|record| record.record.key == first_key)
                .map(|record| record.record.text.as_str()),
            Some("stale first checkpoint\n")
        );
        assert_eq!(
            recovered
                .iter()
                .find(|record| record.record.key == second_key)
                .map(|record| record.record.text.as_str()),
            Some("stale second checkpoint\n")
        );
        assert!(
            !recovered
                .iter()
                .any(|record| record.record.key == first_live_key
                    || record.record.key == second_live_key)
        );
    }

    #[gpui_kit::test]
    fn save_as_collision_preserves_independent_startup_recovery(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.md");
        let destination = dir.path().join("destination.md");
        let saved_source_text = "saved source bytes\n";
        let old_destination_text = "independent destination checkpoint\n";
        fs::write(&source, saved_source_text).unwrap();
        fs::write(&destination, "original destination bytes\n").unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &destination, old_destination_text);
        let overwrite = Arc::new(
            mt_core::document::io::SaveAsOverwriteAuthorization::capture(&destination).unwrap(),
        );
        let startup = populated_startup_recovery(store.clone());
        let destination_for_save = destination.clone();
        let (workspace, cx) = open_test_workspace_with_startup_recovery_inspection(
            cx,
            Some(source),
            move || startup,
            move |workspace, window, cx| {
                let document_id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    document_id,
                    destination_for_save,
                    SaveAsMode::Overwrite(overwrite),
                    window,
                    cx,
                );
            },
        );

        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&destination).unwrap(), saved_source_text);
        let editor_text = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(!document.is_dirty());
            document.text(app)
        });
        assert_eq!(editor_text, saved_source_text);
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(
                workspace.tabs.len(),
                2,
                "both saved and recovered text need a tab"
            );
            let recovered = workspace.document_at(1).unwrap().read(app);
            assert_eq!(recovered.text(app), old_destination_text);
            assert!(recovered.is_dirty());
            assert!(recovered.is_externally_changed());
        });

        let retained_destination_text = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .find(|record| record.record.key == RecoveryKey::for_path(&destination))
            .map(|record| record.record.text);
        assert_eq!(
            retained_destination_text.as_deref(),
            Some(old_destination_text)
        );
    }

    #[gpui_kit::test]
    fn saved_preview_is_kept_until_startup_retirement_is_durable(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first-preview.md");
        let second = dir.path().join("second-preview.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &first, "first checkpoint\n");
        let (workspace, cx) = open_test_workspace_with(cx, None);
        cx.run_until_parked();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file_as(first.clone(), true, window, cx);
                workspace.recovery_flow.recovery = None;
                workspace.recovery_flow.startup_recovery_pending = true;
            });
        });
        replace_document(&workspace, 0, "saved preview text\n", cx);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file_as(second.clone(), true, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.tabs.index_of(&first).is_some());
            assert!(workspace.tabs.index_of(&second).is_some());
        });
        complete_startup_with_store(&workspace, store.clone(), cx);
        cx.run_until_parked();
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn preview_remains_while_retirement_is_queued_behind_an_old_owner(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("queued-preview-first.md");
        let second = dir.path().join("queued-preview-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace_with_recovery_store(cx, None, store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file_as(first.clone(), true, window, cx);
            });
        });
        let key = RecoveryKey::for_path(&first);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_retirements
                .insert(key.clone(), old);
            workspace
                .recovery_flow
                .pending_recovery_retirements
                .insert(key, None);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file_as(second.clone(), true, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(
                workspace.tabs.index_of(&first).is_some(),
                "a queued retirement must keep its preview even while an older owner exists"
            );
            assert!(workspace.tabs.index_of(&second).is_some());
        });
    }

    #[gpui_kit::test]
    fn unavailable_startup_store_keeps_waiting_discard_open(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-without-store.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });
        replace_document(&workspace, 0, "must remain open\n", cx);
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(
                    StartupRecovery {
                        recovery: None,
                        recovery_error: Some("recovery unavailable".into()),
                        ..StartupRecovery::default()
                    },
                    startup_targets,
                    window,
                    cx,
                );
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(!workspace.recovery_flow.startup_recovery_pending);
            assert!(workspace.pending_startup_destructive.is_none());
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Recovery storage is unavailable, so its checkpoint could not be cleared. The document remains open."
                )
            );
        });
        assert_eq!(document_text(&workspace, 0, cx), "must remain open\n");
    }

    #[gpui_kit::test]
    fn dirty_close_cancel_preserves_the_tab_and_exact_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "keep 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            1
        );
        assert_eq!(document_text(&workspace, 0, cx), edited);
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
    }

    #[gpui_kit::test]
    fn failed_save_during_close_keeps_the_tab_and_exact_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conflict.md");
        fs::write(&path, "disk one\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "my exact edit\n";
        replace_document(&workspace, 0, edited, cx);
        fs::write(&path, "disk two\n").unwrap();

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            1
        );
        assert_eq!(document_text(&workspace, 0, cx), edited);
        assert_eq!(fs::read_to_string(path).unwrap(), "disk two\n");
    }

    #[gpui_kit::test]
    fn save_action_refuses_missing_source_and_preserves_exact_editor_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "keep this exact text 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::remove_file(&path).unwrap();

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert!(!path.exists(), "Ctrl-S must not recreate a missing source");
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            true,
            "The source path no longer exists. Recreate it or Save As.",
            cx,
        );
    }

    #[cfg(target_os = "windows")]
    #[gpui_kit::test]
    fn save_action_refuses_retargeted_symlink_without_overwriting_either_target(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target-a.md");
        let alternate = dir.path().join("target-b.md");
        let link = dir.path().join("shared.md");
        fs::write(&target, "target A\n").unwrap();
        fs::write(&alternate, "target B\n").unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping workspace save symlink test: {error}");
            return;
        }
        let (workspace, cx) = open_test_workspace(cx, link.clone());
        let edited = "keep symlink editor text 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::remove_file(&link).unwrap();
        std::os::windows::fs::symlink_file(&alternate, &link).unwrap();

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&target).unwrap(), "target A\n");
        assert_eq!(fs::read_to_string(&alternate).unwrap(), "target B\n");
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            true,
            "The source path or symbolic-link target changed. Save As to preserve both versions.",
            cx,
        );
    }

    #[gpui_kit::test]
    fn save_action_refuses_decode_loss_without_changing_original_bytes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.md");
        let original = b"\xEF\xBB\xBFvalid \xFF byte\n";
        fs::write(&path, original).unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "keep decoded editor text 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), original);
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            false,
            "The original bytes could not be decoded exactly. Convert to UTF-8 or Save As.",
            cx,
        );
    }

    #[gpui_kit::test]
    fn save_action_refuses_unrepresentable_text_without_changing_gbk_bytes(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-gbk.txt");
        let original = b"\xD6\xD0\xCE\xC4\r\n";
        fs::write(&path, original).unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "中文 with emoji \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), original);
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            false,
            "The editor text cannot be represented as GBK. Convert to UTF-8 or Save As.",
            cx,
        );
    }

    #[gpui_kit::test]
    fn document_save_actions_compose_overwrite_and_utf8_conversion(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.md");
        fs::write(&path, b"\xEF\xBB\xBFvalid \xFF byte\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "exact editor text \u{4e2d}\u{6587} \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::write(&path, "external version\n").unwrap();
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();

        document.update(cx, |document, cx| {
            assert!(!document.save(SaveMode::Normal, cx));
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.status.as_deref(),
                Some("This file changed on disk. Reload or overwrite from the banner.")
            );
        });

        document.update(cx, |document, cx| {
            assert!(!document.save(SaveMode::Overwrite, cx));
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "The original bytes could not be decoded exactly. Convert to UTF-8 or Save As."
                )
            );
        });

        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::ConvertToUtf8, cx));
        });
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        document.read_with(cx, |document, app| {
            assert!(!document.is_dirty());
            assert_eq!(document.text(app), edited);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.status.as_deref(), Some("Saved"));
        });
    }

    #[gpui_kit::test]
    fn editing_after_overwrite_authorization_requires_a_new_overwrite_decision(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.md");
        fs::write(&path, b"\xEF\xBB\xBFvalid \xFF byte\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let first_edit = "first editor text \u{4e2d}\u{6587} \u{1f680}\n";
        replace_document(&workspace, 0, first_edit, cx);
        let external = "external version\n";
        fs::write(&path, external).unwrap();
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();

        document.update(cx, |document, cx| {
            assert!(!document.save(SaveMode::Normal, cx));
            assert!(!document.save(SaveMode::Overwrite, cx));
        });
        let second_edit = "newer editor text \u{4e2d}\u{6587} \u{1f680}\n";
        replace_document(&workspace, 0, second_edit, cx);

        document.update(cx, |document, cx| {
            assert!(!document.save(SaveMode::ConvertToUtf8, cx));
        });
        cx.run_until_parked();

        assert_eq!(fs::read_to_string(&path).unwrap(), external);
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert_eq!(document.text(app), second_edit);
        });
    }

    #[gpui_kit::test]
    fn auto_reload_cannot_replace_a_dirty_editor(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "editor 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::write(path, "external rewrite\n").unwrap();
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();

        let started = document.update(cx, |document, cx| document.reload_if_clean(cx));

        assert!(!started);
        assert_eq!(
            document.read_with(cx, |document, app| document.text(app)),
            edited
        );
    }

    #[gpui_kit::test]
    fn watcher_auto_reload_waits_for_startup_recovery_and_preserves_conflict(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup-watcher.md");
        fs::write(&path, "disk before startup\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "unseen recovered text\n");
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });
        cx.update(|_, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.watch_auto_reload = true;
            });
        });

        fs::write(&path, "external rewrite during startup\n").unwrap();
        let startup = populated_startup_recovery(store);
        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(dir.path(), &[Change::Modified(path.clone())], cx);
        });
        cx.run_until_parked();

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert_eq!(document.text(app), "disk before startup\n");
            assert!(document.is_externally_changed());
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, startup_targets, window, cx);
            });
        });
        document.read_with(cx, |document, app| {
            assert_eq!(document.text(app), "unseen recovered text\n");
            assert!(document.is_dirty());
            assert!(
                document.is_externally_changed(),
                "the startup recovery must retain the watcher conflict"
            );
        });
    }

    #[gpui_kit::test]
    fn clean_auto_reload_deletion_enters_the_missing_source_state(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("removed-clean.md");
        let text = "disk text stays visible\n";
        fs::write(&path, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        cx.update(|_, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.watch_auto_reload = true;
            });
        });
        fs::remove_file(&path).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(dir.path(), &[Change::Removed(path)], cx);
        });
        cx.run_until_parked();

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert!(!document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), text);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.status.as_deref(),
                Some("The source path no longer exists. Recreate it or Save As.")
            );
        });
    }

    #[cfg(target_os = "windows")]
    #[gpui_kit::test]
    fn resolved_symlink_target_change_marks_dirty_document_without_replacing_text(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("shared-target.md");
        let link = dir.path().join("shared-link.md");
        fs::write(&target, "disk\n").unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping symlink watcher test: {error}");
            return;
        }

        let (workspace, cx) = open_test_workspace(cx, link);
        let edited = "editor 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(dir.path(), &[Change::Modified(target.clone())], cx);
        });

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), edited);
        });
    }

    #[cfg(feature = "model-transport")]
    #[gpui_kit::test]
    fn opening_and_scanning_with_model_configured_sends_no_request(cx: &mut TestAppContext) {
        use mt_core::model::{EndpointIdentity, Provider};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
        let endpoint = EndpointIdentity::parse(Provider::OpenAiChat, Some(&base_url)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local-only-open.md");
        fs::write(&path, "Private content stays local\n").unwrap();

        cx.update(|app| {
            gpui_kit::init(app);
            crate::settings::AppSettings::init(app);
            crate::settings::AppSettings::update(app, |settings| {
                settings.show_welcome_on_startup = false;
                settings.model_provider = Provider::OpenAiChat.key().into();
                settings.model_name = "configured-model".into();
                settings.model_base_url = base_url;
            });
            super::init(app);
            crate::credentials::AppCredentialVault::global(app)
                .replace_session(
                    endpoint.credential_target().to_string(),
                    "synthetic-local-only-credential".into(),
                )
                .unwrap();
        });
        let captured = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| {
                    Workspace::new_with_startup_recovery(
                        Some(path),
                        StartupRecovery::default,
                        window,
                        cx,
                    )
                });
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_kit::component::Root::new(workspace, window, cx)
            }
        });
        let workspace = captured.borrow().clone().expect("the Workspace entity");
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| workspace.rescan_harness(cx));
        cx.run_until_parked();

        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "opening, rendering, and Skill discovery must stay local"
        );
    }

    #[cfg(feature = "model-transport")]
    #[gpui_kit::test]
    fn cancelling_translation_consent_sends_no_request(cx: &mut TestAppContext) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent-cancel.md");
        fs::write(&path, "Hello from a private document\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        configure_test_translation(cx, &base_url);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_translate_document(&super::TranslateDocument, window, cx);
            });
        });

        assert!(cx.has_pending_prompt());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "preparing disclosure must not open a connection"
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "cancelled consent must keep the endpoint untouched"
        );
        workspace.read_with(cx, |workspace, _| assert!(!workspace.translating));
    }

    #[cfg(feature = "model-transport")]
    #[gpui_kit::test]
    fn changing_the_document_invalidates_pending_translation_consent(cx: &mut TestAppContext) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent-stale.md");
        fs::write(&path, "Original private text\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        configure_test_translation(cx, &base_url);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_translate_document(&super::TranslateDocument, window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        replace_document(&workspace, 0, "Changed while the prompt was open\n", cx);

        cx.simulate_prompt_answer("Send");
        cx.run_until_parked();

        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "approval for an obsolete snapshot must not open a connection"
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.translating);
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "The document changed while approval was pending. Review the updated scope and try again."
                )
            );
        });
    }

    #[cfg(feature = "model-transport")]
    #[gpui_kit::test]
    fn approving_translation_consent_sends_one_frozen_request(cx: &mut TestAppContext) {
        let (base_url, received, request_count) = one_shot_translation_server();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent-approve.md");
        fs::write(&path, "Hello\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        configure_test_translation(cx, &base_url);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_translate_document(&super::TranslateDocument, window, cx);
            });
        });

        assert!(cx.has_pending_prompt());
        assert_eq!(request_count.load(Ordering::SeqCst), 0);
        cx.simulate_prompt_answer("Send");
        cx.run_until_parked();
        received
            .recv_timeout(Duration::from_secs(10))
            .expect("the approved request reached the configured endpoint");
        cx.run_until_parked();

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert_eq!(document.text(app), "Bonjour\n");
        });
    }

    #[gpui_kit::test]
    fn stale_transformation_result_keeps_the_newer_editor_revision_and_text(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("translation.md");
        fs::write(&path, "revision N\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let snapshot = document.read_with(cx, |document, app| document.async_snapshot(app));

        replace_document(&workspace, 0, "revision N+1 中文 \u{1f680}\n", cx);
        let revision_after_edit = document.read_with(cx, |document, _| document.revision());

        let applied = cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text_if_current(
                    &snapshot,
                    "stale translation\n".into(),
                    window,
                    cx,
                )
            })
        });

        assert!(!applied);
        document.read_with(cx, |document, app| {
            assert_eq!(document.revision(), revision_after_edit);
            assert_eq!(document.text(app), "revision N+1 中文 \u{1f680}\n");
        });
    }

    #[gpui_kit::test]
    fn save_as_rejects_a_transformation_from_the_previous_source_identity(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("translation.md");
        let saved_as = dir.path().join("translation.html");
        let text = "same revision and text\n";
        fs::write(&original, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, original);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let snapshot = document.read_with(cx, |document, app| document.async_snapshot(app));

        document.update(cx, |document, cx| {
            assert_eq!(
                document.save_as(&saved_as, SaveAsMode::CreateOnly, cx),
                SaveAsOutcome::Saved
            );
        });
        let applied = cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text_if_current(
                    &snapshot,
                    "stale translation\n".into(),
                    window,
                    cx,
                )
            })
        });

        assert!(!applied);
        document.read_with(cx, |document, app| {
            assert_eq!(document.source_path(), Some(saved_as.as_path()));
            assert_eq!(document.document().doc_type(), mt_core::DocType::Html);
            assert_eq!(document.text(app), text);
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui_kit::test]
    fn trusted_mdx_save_as_html_is_restricted_before_the_new_web_payload(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("trusted.mdx");
        let saved_as = dir.path().join("restricted.html");
        let text = "<!doctype html><html><body><script>window.ran = true</script></body></html>";
        fs::write(&original, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, original);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            document.set_layout(Layout::Web, cx);
            document.set_trust(Trust::Trusted, cx);
        });
        let id = document.read_with(cx, |document, _| document.id());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        document.read_with(cx, |document, _| {
            assert_eq!(document.source_path(), Some(saved_as.as_path()));
            assert_eq!(document.document().doc_type(), mt_core::DocType::Html);
            assert_eq!(document.trust(), Trust::Restricted);
            let payload = document.web_html().expect("the rebuilt HTML payload");
            assert!(!payload.starts_with("file://"));
            assert!(web::to_data_url(payload).starts_with("data:text/html;charset=utf-8,"));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui_kit::test]
    fn trusted_html_path_only_save_as_rebuilds_as_restricted_data_url(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("trusted-before.html");
        let saved_as = dir.path().join("restricted-after.html");
        let text = "<!doctype html><html><body><img src=local.png></body></html>";
        fs::write(&original, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, original);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            document.set_trust(Trust::Trusted, cx);
        });
        document.read_with(cx, |document, _| {
            assert_eq!(document.trust(), Trust::Trusted);
            assert!(document.web_html().unwrap().starts_with("file://"));
        });
        let id = document.read_with(cx, |document, _| document.id());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        document.read_with(cx, |document, _| {
            assert_eq!(document.source_path(), Some(saved_as.as_path()));
            assert_eq!(document.document().doc_type(), mt_core::DocType::Html);
            assert_eq!(document.trust(), Trust::Restricted);
            let payload = document.web_html().expect("the rebuilt HTML payload");
            assert!(!payload.starts_with("file://"));
            assert!(web::to_data_url(payload).starts_with("data:text/html;charset=utf-8,"));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui_kit::test]
    fn failed_save_as_preserves_trust_and_the_existing_web_payload(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("trusted.html");
        let failed_path = dir.path().join("missing-parent").join("failed.html");
        fs::write(&original, "<!doctype html><p>trusted</p>").unwrap();
        let (workspace, cx) = open_test_workspace(cx, original.clone());
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            document.set_trust(Trust::Trusted, cx);
        });
        let id = document.read_with(cx, |document, _| document.id());
        let before = document.read_with(cx, |document, _| document.web_html().unwrap().to_string());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    id,
                    failed_path.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });

        document.read_with(cx, |document, _| {
            assert_eq!(document.source_path(), Some(original.as_path()));
            assert_eq!(document.trust(), Trust::Trusted);
            assert_eq!(document.web_html(), Some(before.as_str()));
        });
        assert!(!failed_path.exists());
    }

    #[gpui_kit::test]
    fn markdown_save_as_preserves_content_layout_and_restricted_payload(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("before.md");
        let saved_as = dir.path().join("after.md");
        let text = "# Exact Markdown\n\nbody\n";
        fs::write(&original, text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, original);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            document.set_layout(Layout::Web, cx);
        });
        let id = document.read_with(cx, |document, _| document.id());
        let before = document.read_with(cx, |document, _| document.web_html().unwrap().to_string());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        document.read_with(cx, |document, app| {
            assert_eq!(document.source_path(), Some(saved_as.as_path()));
            assert_eq!(document.document().doc_type(), mt_core::DocType::Markdown);
            assert_eq!(document.layout(), Layout::Web);
            assert_eq!(document.trust(), Trust::Restricted);
            assert_eq!(document.text(app), text);
            assert_eq!(document.web_html(), Some(before.as_str()));
            assert!(
                web::to_data_url(document.web_html().unwrap())
                    .starts_with("data:text/html;charset=utf-8,")
            );
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui_kit::test]
    fn save_retries_a_failed_durable_recovery_retirement(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save-retirement-retry.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let edited = "saved before retirement retry\n";
        replace_document(&workspace, 0, edited, cx);
        write_recovery_checkpoint(&store, &path, "older checkpoint\n");
        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&document.id())
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(now);
            let attempt = RecoveryAttempt {
                token: state
                    .token
                    .clone()
                    .expect("a ready test store must provide a recovery token"),
                content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                timing: schedule.checkpoint_dispatched(now).unwrap(),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            state.schedule = schedule;
            state.in_flight = Some(attempt);
        });
        store.fail_next_persist_for_test();

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(document.is_dirty());
            assert_eq!(document.text(app), edited);
            assert!(
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&document.id())
                    .is_some_and(|state| state.in_flight.is_none())
            );
            assert!(
                workspace.status.as_deref().is_some_and(
                    |status| status.contains("Could not clear the recovery checkpoint")
                )
            );
        });
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
        assert_eq!(
            store.recover().unwrap().records[0].record.text,
            "older checkpoint\n"
        );

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, edited);
    }

    #[gpui_kit::test]
    fn discard_proceeds_after_the_record_is_retired_even_if_cleanup_fails(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-cleanup-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        replace_document(&workspace, 0, "discarded after rename\n", cx);
        let checkpoint = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_at(0)
                .unwrap()
                .read(app)
                .recovery_checkpoint(app)
        });
        store
            .checkpoint(&checkpoint, &HashSet::from([checkpoint.key.clone()]))
            .unwrap();
        store.fail_next_delete_for_test();

        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.tabs.is_empty());
            assert!(workspace.status.as_deref().is_some_and(|status| {
                status.contains("checkpoint was cleared, but cleanup remains pending")
            }));
        });
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn discard_keeps_the_tab_open_when_recovery_retirement_fails(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-retirement-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let edited = "discard only after durable retirement\n";
        replace_document(&workspace, 0, edited, cx);
        let checkpoint = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_at(0)
                .unwrap()
                .read(app)
                .recovery_checkpoint(app)
        });
        store
            .checkpoint(&checkpoint, &HashSet::from([checkpoint.key.clone()]))
            .unwrap();
        store.fail_next_persist_for_test();

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_retries
                    .contains(&checkpoint.key)
            );
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        assert_eq!(store.recover().unwrap().records.len(), 1);

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert!(store.recover().unwrap().records.is_empty());
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirement_retries
                    .contains(&checkpoint.key)
            );
        });
    }

    #[gpui_kit::test]
    fn unrelated_failed_retirement_does_not_block_clean_close_tab(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("failed-retirement.md");
        let second = dir.path().join("clean-close.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });

        replace_document(&workspace, 0, "saved first text\n", cx);
        let first_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let (first_id, checkpoint) = first_document.read_with(cx, |document, app| {
            (document.id(), document.recovery_checkpoint(app))
        });
        store
            .checkpoint(&checkpoint, &HashSet::from([checkpoint.key.clone()]))
            .unwrap();
        store.fail_next_persist_for_test();
        first_document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .get(&checkpoint.key),
                Some(&Some(first_id))
            );
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_retries
                    .contains(&checkpoint.key)
            );
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_close_tab(1, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.tabs.index_of(&first).is_some());
            assert!(workspace.tabs.index_of(&second).is_none());
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
        });
    }

    #[gpui_kit::test]
    fn close_tab_waits_for_its_pre_save_as_startup_key(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("before-save-as.md");
        let saved_as = dir.path().join("after-save-as.md");
        fs::write(&original, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, original.clone());
        let id = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).id()
        });
        let original_key = RecoveryKey::for_path(&original);
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
            workspace
                .recovery_flow
                .startup_recovery_keys
                .insert(id, original_key.clone());
        });
        replace_document(&workspace, 0, "saved elsewhere\n", cx);
        let live_key = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).recovery_key()
        });
        assert_ne!(live_key, original_key);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .get(&live_key),
                Some(&Some(id))
            );
            assert_eq!(
                workspace.recovery_flow.startup_recovery_keys.get(&id),
                Some(&original_key),
                "the old startup key must stay independent from the saved live incarnation"
            );
            assert!(workspace.tabs.index_of(&saved_as).is_some());
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_close_tab(0, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            let pending = workspace
                .pending_startup_destructive
                .as_ref()
                .expect("the live-key retirement must delay the target tab close");
            assert!(pending.keys.contains(&(live_key, Some(id))));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), "saved elsewhere\n");
    }

    #[gpui_kit::test]
    fn saving_during_startup_clears_the_save_as_recovery_key(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let id = document.read_with(cx, |document, _| document.id());
        let startup_key = RecoveryKey::for_path(&dir.path().join("startup.md"));
        let save_as_key = RecoveryKey::for_path(&dir.path().join("save-as.md"));

        replace_document(&workspace, 0, "saved\n", cx);
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
            workspace
                .recovery_flow
                .startup_recovery_keys
                .insert(id, startup_key.clone());
            workspace
                .recovery_flow
                .save_as_recovery_keys
                .insert(id, save_as_key.clone());
            // Exercise the fallback selection directly; an active schedule
            // normally takes precedence in retire_document_recovery.
            workspace.recovery_flow.recovery_schedules.remove(&id);
        });

        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });

        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .recovery_flow
                    .startup_recovery_keys
                    .contains_key(&id),
                "saving must clear the startup recovery key"
            );
            assert!(
                !workspace
                    .recovery_flow
                    .save_as_recovery_keys
                    .contains_key(&id),
                "saving must clear the stale Save As recovery key"
            );
            assert_eq!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .get(&save_as_key),
                Some(&Some(id)),
                "a Save As key identifies the dirty source and takes precedence"
            );
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&startup_key),
                "the superseded startup key must not replace the Save As key"
            );
        });
    }

    #[gpui_kit::test]
    fn clean_save_as_before_startup_scan_preserves_independent_recovery(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("clean-before-scan.md");
        let saved_as = dir.path().join("saved-before-scan.md");
        let clean_disk_bytes = b"# Clean source\r\nExact source bytes.\r\n";
        let old_checkpoint = "# Earlier A checkpoint\nRecovered text stays available.\n";
        fs::write(&original, clean_disk_bytes).unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &original, old_checkpoint);

        let save_as_completed = Arc::new(AtomicBool::new(false));
        let scan_after_save_as = save_as_completed.clone();
        let store_for_startup = store.clone();
        let destination_for_save = saved_as.clone();
        let (workspace, cx) = open_test_workspace_with_startup_recovery_inspection(
            cx,
            Some(original.clone()),
            move || {
                assert!(
                    scan_after_save_as.load(Ordering::Acquire),
                    "the deferred startup scan must run after Save As"
                );
                populated_startup_recovery(store_for_startup)
            },
            move |workspace, window, cx| {
                let id = workspace.document_at(0).unwrap().read(cx).id();
                workspace.finish_save_as(
                    id,
                    destination_for_save,
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
                save_as_completed.store(true, Ordering::Release);
            },
        );
        cx.run_until_parked();

        assert_eq!(fs::read(&original).unwrap().as_slice(), clean_disk_bytes);
        assert_eq!(fs::read(&saved_as).unwrap().as_slice(), clean_disk_bytes);

        let records = store.recover().unwrap().records;
        assert_eq!(
            records.len(),
            1,
            "the independent A checkpoint must remain durable"
        );
        let old_record = records
            .iter()
            .find(|record| record.record.key == RecoveryKey::for_path(&original))
            .expect("the old A checkpoint must not be consumed by saving to C");
        assert_eq!(old_record.record.text, old_checkpoint);

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let visible_old_checkpoint = workspace
                .tabs
                .iter()
                .filter_map(|tab| {
                    let document = tab.payload.view.read(app);
                    (document.text(app) == old_checkpoint).then(|| {
                        (
                            document.source_path().map(Path::to_path_buf),
                            document.is_dirty(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(visible_old_checkpoint.len(), 1);
            assert_eq!(
                visible_old_checkpoint[0].0.as_deref(),
                Some(original.as_path())
            );
            assert!(visible_old_checkpoint[0].1);
        });
    }

    #[gpui_kit::test]
    fn startup_edit_before_deferred_scan_preserves_old_checkpoint(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("edited-before-scan.md");
        let disk_text = "clean A on disk\n";
        let old_checkpoint = "older A checkpoint\n";
        let live_text = "A edited before the startup scan\n";
        fs::write(&original, disk_text).unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &original, old_checkpoint);

        let edited_before_scan = Arc::new(AtomicBool::new(false));
        let scan_after_edit = edited_before_scan.clone();
        let store_for_startup = store.clone();
        let (workspace, cx) = open_test_workspace_with_startup_recovery_inspection(
            cx,
            Some(original.clone()),
            move || {
                assert!(
                    scan_after_edit.load(Ordering::Acquire),
                    "the startup scan must follow the user's edit"
                );
                populated_startup_recovery(store_for_startup)
            },
            move |workspace, window, cx| {
                let document = workspace.document_at(0).cloned().unwrap();
                document.update(cx, |document, cx| {
                    document.replace_text(live_text.to_string(), window, cx);
                });
                edited_before_scan.store(true, Ordering::Release);
            },
        );
        cx.run_until_parked();

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        let original_key = RecoveryKey::for_path(&original);
        let records = store.recover().unwrap().records;
        assert_eq!(
            records.len(),
            2,
            "the edit must not overwrite A's older checkpoint"
        );
        let old_record = records
            .iter()
            .find(|record| record.record.key == original_key)
            .expect("A's original recovery key must remain durable");
        assert_eq!(old_record.record.text, old_checkpoint);
        let live_record = records
            .iter()
            .find(|record| record.record.text == live_text)
            .expect("the edited A buffer must receive its own checkpoint");
        assert_ne!(live_record.record.key, original_key);
        let live_key = live_record.record.key.clone();

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let live = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(path) if path == &original => {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the current A document must stay open");
            let live = live.read(app);
            assert_eq!(live.text(app), live_text);
            assert_eq!(live.recovery_key(), live_key);
            assert!(live.is_dirty());

            let recovered = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::Recovered(key)
                        if key == &original_key =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the old A checkpoint must be presented separately");
            let recovered = recovered.read(app);
            assert_eq!(recovered.text(app), old_checkpoint);
            assert!(recovered.is_dirty());
        });
    }

    #[gpui_kit::test]
    fn clean_document_opened_during_startup_retires_its_old_key_after_save_as(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("late-open-before-save-as.md");
        let saved_as = dir.path().join("late-open-after-save-as.md");
        fs::write(&original, "disk before startup\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &original, "old-path checkpoint\n");
        let startup = populated_startup_recovery(store.clone());
        let (workspace, cx) = open_test_workspace_with(cx, None);
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(original.clone(), window, cx);
            });
        });
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let id = document.read_with(cx, |document, _| document.id());
        let original_key = RecoveryKey::for_path(&original);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.recovery_flow.startup_recovery_keys.get(&id),
                Some(&original_key),
                "a clean tab opened during startup must retain its original key"
            );
        });

        cx.update(|_, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.watch_auto_reload = true;
            });
        });
        fs::write(&original, "external rewrite during startup\n").unwrap();
        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(dir.path(), &[Change::Modified(original.clone())], cx);
        });
        document.read_with(cx, |document, app| {
            assert!(!document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), "disk before startup\n");
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .get(&original_key),
                Some(&Some(id))
            );
            assert!(workspace.tabs.index_of(&original).is_none());
            assert!(workspace.tabs.index_of(&saved_as).is_some());
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, HashMap::new(), window, cx);
                assert_eq!(workspace.tabs.len(), 1);
                assert!(workspace.tabs.index_of(&original).is_none());
                assert!(workspace.tabs.index_of(&saved_as).is_some());
                assert!(
                    workspace
                        .recovery_flow
                        .recovery_retirements
                        .contains_key(&original_key)
                );
            });
        });
        cx.run_until_parked();

        assert!(store.recover().unwrap().records.is_empty());
        assert_eq!(
            fs::read_to_string(saved_as).unwrap(),
            "disk before startup\n"
        );
    }

    #[gpui_kit::test]
    fn full_workspace_actions_include_all_pending_keys_in_one_batch(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("full-action-first.md");
        let second = dir.path().join("full-action-second.md");
        let unknown = dir.path().join("unknown-origin.md");
        let replacement = dir.path().join("replacement");
        fs::write(&first, "first\n").unwrap();
        fs::write(&second, "second\n").unwrap();
        fs::create_dir(&replacement).unwrap();
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });
        let (first_id, second_id) = workspace.read_with(cx, |workspace, app| {
            (
                workspace.document_at(0).unwrap().read(app).id(),
                workspace.document_at(1).unwrap().read(app).id(),
            )
        });
        let first_key = RecoveryKey::for_path(&first);
        let second_key = RecoveryKey::for_path(&second);
        let unknown_key = RecoveryKey::for_path(&unknown);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .insert(first_key.clone(), Some(first_id));
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .insert(second_key.clone(), Some(second_id));
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .insert(unknown_key.clone(), None);

                let all =
                    HashSet::from([first_key.clone(), second_key.clone(), unknown_key.clone()]);
                for action in [
                    DestructiveAction::CloseWindow,
                    DestructiveAction::ReplaceWorkspace(replacement.clone()),
                ] {
                    let selected = workspace
                        .pending_recovery_keys(&action)
                        .into_iter()
                        .map(|(key, _)| key)
                        .collect::<HashSet<_>>();
                    assert_eq!(selected, all);
                }
                let close_tab_keys = workspace
                    .pending_recovery_keys(&DestructiveAction::CloseTab(first_id))
                    .into_iter()
                    .map(|(key, _)| key)
                    .collect::<HashSet<_>>();
                assert_eq!(
                    close_tab_keys,
                    HashSet::from([first_key.clone(), unknown_key.clone()]),
                    "unknown-origin work must remain fail-closed"
                );

                let request = DestructiveRequest::new(
                    DestructiveAction::ReplaceWorkspace(replacement.clone()),
                    &workspace.lifecycle_documents(cx),
                );
                let DestructiveResolution::Proceed(action) = request.initial_resolution() else {
                    panic!("clean documents must not prompt before workspace replacement");
                };
                workspace.perform_after_discard_retirement(request, action, Vec::new(), window, cx);

                let batch = workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .get(&first_key)
                    .cloned()
                    .expect("the first pending key must enter the batch");
                assert_eq!(
                    workspace
                        .recovery_flow
                        .recovery_retirement_batches
                        .get(&second_key),
                    Some(&batch)
                );
                assert_eq!(
                    workspace
                        .recovery_flow
                        .recovery_retirement_batches
                        .get(&unknown_key),
                    Some(&batch)
                );
                assert_eq!(workspace.recovery_flow.recovery_retirement_batches.len(), 3);
            });
        });
    }

    #[gpui_kit::test]
    fn matched_old_completion_replays_a_queued_save_retirement(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queued-save-replay.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        write_recovery_checkpoint(&store, &path, "queued checkpoint\n");
        let key = RecoveryKey::for_path(&path);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_retirements
                .insert(key.clone(), old.clone());
        });
        let old_completion = store.complete_retirement(old.clone()).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, None, cx);
        });
        let fresh = workspace.read_with(cx, |workspace, _| {
            let fresh = workspace
                .recovery_flow
                .recovery_retirements
                .get(&key)
                .cloned()
                .expect("the second Save must install a fresh retirement owner");
            assert_ne!(fresh, old);
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
            fresh
        });

        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_retirement(key.clone(), old, None, Ok(old_completion), cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.recovery_flow.recovery_retirements.get(&key),
                Some(&fresh)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
        });
    }

    #[gpui_kit::test]
    fn second_save_replaces_a_stale_ui_retirement_owner(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("second-save-stale-owner.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        write_recovery_checkpoint(&store, &path, "first saved checkpoint\n");
        let key = RecoveryKey::for_path(&path);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_retirements
                .insert(key.clone(), old.clone());
        });

        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, None, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.recovery_flow.recovery_retirements.get(&key),
                Some(&old)
            );
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
        });
        let old_completion = store.complete_retirement(old.clone()).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_retirement(
                key.clone(),
                old.clone(),
                None,
                Ok(old_completion),
                cx,
            );
        });
        workspace.read_with(cx, |workspace, _| {
            let fresh = workspace
                .recovery_flow
                .recovery_retirements
                .get(&key)
                .expect("the matched old completion must replay the queued Save");
            assert_ne!(fresh, &old);
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
        });
    }

    #[gpui_kit::test]
    fn edit_cancels_only_the_queued_retirement_intent(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edit-cancels-queued-retirement.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let key = document.read_with(cx, |document, _| document.recovery_key());
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_retirements
                .insert(key.clone(), old.clone());
        });
        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, None, cx);
        });

        cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text("new edit cancels only the queue\n".into(), window, cx);
            });
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
            assert_eq!(
                workspace.recovery_flow.recovery_retirements.get(&key),
                Some(&old)
            );
        });
    }

    #[gpui_kit::test]
    fn stale_batch_takeover_resumes_the_destructive_action(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-batch-takeover.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        replace_document(&workspace, 0, "discard after takeover\n", cx);
        let (id, checkpoint) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (document.id(), document.recovery_checkpoint(app))
        });
        let key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([key.clone()]))
            .unwrap();
        let old_batch = store.begin_retirements([key.clone()]).unwrap();
        let old_completion = store.complete_retirements(old_batch.clone()).unwrap();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseTab(id), &documents);
                assert!(matches!(
                    request.decide(DirtyDecision::Discard, None, &documents),
                    DestructiveResolution::Proceed(_)
                ));
                workspace.pending_destructive = Some(request);
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .insert(key.clone(), old_batch.clone());
                workspace.invalidate_recovery(&key, Some(id), cx);
                workspace.finish_discard_retirements(
                    vec![(key.clone(), Some(id))],
                    old_batch.clone(),
                    Ok(old_completion),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()),
            "a stale batch callback must resume the action through the fresh owner"
        );
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn replay_persist_failure_keeps_destructive_action_open_until_retry(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replay-persist-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        replace_document(&workspace, 0, "keep open through replay retry\n", cx);
        let (id, checkpoint) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (document.id(), document.recovery_checkpoint(app))
        });
        let key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([key.clone()]))
            .unwrap();
        let old_batch = store.begin_retirements([key.clone()]).unwrap();
        let old_completion = store.complete_retirements(old_batch.clone()).unwrap();
        store.fail_next_persist_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseTab(id), &documents);
                assert!(matches!(
                    request.decide(DirtyDecision::Discard, None, &documents),
                    DestructiveResolution::Proceed(_)
                ));
                workspace.pending_destructive = Some(request);
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .insert(key.clone(), old_batch.clone());
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .insert(key.clone(), Some(id));
                let content_identity = workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&id)
                    .expect("the dirty tab has a checkpoint schedule")
                    .content_identity;
                workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .entry(key.clone())
                    .or_default()
                    .insert(id, content_identity);
                workspace.schedule_recovery_timer(cx);
                workspace.finish_discard_retirements(
                    vec![(key.clone(), Some(id))],
                    old_batch.clone(),
                    Ok(old_completion),
                    window,
                    cx,
                );
            });
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
            assert!(workspace.pending_startup_destructive.is_some());
        });

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert!(workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn newer_edit_rearms_recovery_during_retirement_cleanup(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edit-during-retirement.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let retired_text = "retirement removes this checkpoint\n";
        replace_document(&workspace, 0, retired_text, cx);
        let (id, checkpoint, retired_identity) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            (
                document.id(),
                document.recovery_checkpoint(app),
                state.content_identity,
            )
        });
        let key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([key.clone()]))
            .unwrap();
        store.fail_next_delete_for_test();

        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, Some(id), cx);
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .get(&key)
                    .and_then(|documents| documents.get(&id)),
                Some(&retired_identity)
            );
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirements
                    .contains_key(&key)
            );
        });

        let new_text = "newly authored text remains recoverable\n";
        replace_document(&workspace, 0, new_text, cx);
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_ne!(state.content_identity, retired_identity);
            assert_eq!(
                workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .get(&key)
                    .and_then(|documents| documents.get(&id)),
                Some(&retired_identity),
                "the new content must not release the old identity's suppression"
            );
        });

        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirements
                    .contains_key(&key)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .contains_key(&key)
            );
        });
        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, new_text);
    }

    #[gpui_kit::test]
    fn reopened_same_path_is_not_suppressed_by_retired_document(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reopened-retirement.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        replace_document(&workspace, 0, "old document revision one\n", cx);
        let (old_id, checkpoint, retired_identity) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            (
                document.id(),
                document.recovery_checkpoint(app),
                state.content_identity,
            )
        });
        let key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([key.clone()]))
            .unwrap();
        store.fail_next_delete_for_test();
        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, Some(old_id), cx);
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert_eq!(
                workspace.remove_recovery_schedule(old_id, cx),
                Some(key.clone())
            );
            assert!(workspace.tabs.close(0).is_some());
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_file(path.clone(), window, cx));
            });
        });
        cx.run_until_parked();

        let new_text = "new document at the same path\n";
        replace_document(&workspace, 0, new_text, cx);
        let (new_id, new_identity) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&document.id())
                    .unwrap()
                    .content_identity,
            )
        });
        assert_ne!(new_id, old_id);
        assert_eq!(new_identity, retired_identity);
        workspace.read_with(cx, |workspace, _| {
            let suppressions = workspace
                .recovery_flow
                .recovery_retirement_suppressions
                .get(&key)
                .unwrap();
            assert_eq!(suppressions.get(&old_id), Some(&retired_identity));
            assert!(!suppressions.contains_key(&new_id));
            assert!(
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .contains_key(&new_id)
            );
        });

        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirements
                    .contains_key(&key)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .contains_key(&key)
            );
        });

        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();
        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, new_text);
    }

    #[gpui_kit::test]
    fn answers_authored_during_retirement_block_tab_close(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answers-during-retirement.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let text = "dirty source remains open\n";
        replace_document(&workspace, 0, text, cx);
        let (id, checkpoint, binding) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                document.recovery_checkpoint(app),
                RevisionRequestBinding::new(
                    [1; 32],
                    document.revision(),
                    0,
                    [2; 32],
                    [3; 32],
                    [4; 32],
                ),
            )
        });
        let key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([key.clone()]))
            .unwrap();
        let batch = store.begin_retirements([key.clone()]).unwrap();
        let completion = store.complete_retirements(batch.clone()).unwrap();

        let answers_text = "preserve this answer while closing";
        let revision_recovery = RevisionRecovery::new(
            binding,
            RevisionAnswers::for_recovery(vec![RevisionAnswer::answered(answers_text)]).unwrap(),
        );
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseTab(id), &documents);
                assert!(matches!(
                    request.decide(DirtyDecision::Discard, None, &documents),
                    DestructiveResolution::Proceed(_)
                ));
                let content_identity = workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&id)
                    .expect("the dirty tab has a checkpoint schedule")
                    .content_identity;
                workspace.pending_destructive = Some(request);
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .insert(key.clone(), batch.clone());
                workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .entry(key.clone())
                    .or_default()
                    .insert(id, content_identity);
                workspace.review_flow.store_recovered_revision_record(
                    id,
                    key.clone(),
                    revision_recovery,
                );
                workspace.finish_discard_retirements(
                    vec![(key.clone(), Some(id))],
                    batch.clone(),
                    Ok(completion),
                    window,
                    cx,
                );
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.revision_has_authored_answers_for_document(id, app));
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirement_suppressions
                    .contains_key(&key)
            );
            assert!(
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&id)
                    .is_some_and(|state| { state.content_identity.revision_binding.is_some() })
            );
            let expected_status: String = i18n::t(i18n::Key::RevisionAnswersRetained, app).into();
            assert_eq!(workspace.status.as_deref(), Some(expected_status.as_str()));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
        });

        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();
        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, text);
        let revision = recovered.records[0]
            .record
            .revision
            .as_ref()
            .expect("the answer authored during retirement must be checkpointed");
        assert_eq!(revision.answers().as_slice().len(), 1);
        assert_eq!(
            revision.answers().as_slice()[0],
            RevisionAnswer::answered(answers_text)
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseTab(id), &documents);
                assert!(matches!(
                    request.decide(DirtyDecision::Discard, None, &documents),
                    DestructiveResolution::Proceed(_)
                ));
                workspace.resume_recovery_destructive(
                    super::PendingStartupDestructive {
                        request,
                        keys: vec![(key.clone(), Some(id))],
                    },
                    window,
                    cx,
                );
            });
        });
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.revision_has_authored_answers_for_document(id, app));
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .is_empty()
            );
            assert!(workspace.pending_startup_destructive.is_none());
        });
    }

    #[gpui_kit::test]
    fn marker_write_retry_does_not_suppress_later_batch_cleanup_retry(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("marker-retry-first.md");
        let second = dir.path().join("marker-retry-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });

        replace_document(&workspace, 0, "saved first text\n", cx);
        let first_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let first_checkpoint =
            first_document.read_with(cx, |document, app| document.recovery_checkpoint(app));
        let first_key = first_checkpoint.key.clone();
        store
            .checkpoint(&first_checkpoint, &HashSet::from([first_key.clone()]))
            .unwrap();
        store.fail_next_persist_for_test();
        first_document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_retries
                    .contains(&first_key)
            );
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&first_key)
            );
        });

        let second_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(1).cloned())
            .unwrap();
        cx.update(|window, app| {
            second_document.update(app, |document, cx| {
                document.replace_text("discarded second text\n".into(), window, cx);
            });
        });
        let (second_id, second_checkpoint) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(1).unwrap().read(app);
            (document.id(), document.recovery_checkpoint(app))
        });
        let second_key = second_checkpoint.key.clone();
        store
            .checkpoint(
                &second_checkpoint,
                &HashSet::from([first_key.clone(), second_key.clone()]),
            )
            .unwrap();
        store.fail_next_delete_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseTab(second_id), &documents);
                let DestructiveResolution::Proceed(action) =
                    request.decide(DirtyDecision::Discard, None, &documents)
                else {
                    panic!("the dirty second document must be authorized for discard");
                };
                workspace.perform_after_discard_retirement(
                    request,
                    action,
                    vec![(second_key.clone(), Some(second_id))],
                    window,
                    cx,
                );
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .remove(&first_key);
            });
        });
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .is_empty(),
                "the old marker retry must not strand a later batch cleanup"
            );
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .is_empty()
            );
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirement_retries
                    .is_empty()
            );
        });

        replace_document(&workspace, 0, "dirty after cleanup\n", cx);
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn save_and_discard_clear_the_recovery_record(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        replace_document(&workspace, 0, "saved text\n", cx);
        let saved_checkpoint = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_at(0)
                .unwrap()
                .read(app)
                .recovery_checkpoint(app)
        });
        store
            .checkpoint(
                &saved_checkpoint,
                &HashSet::from([saved_checkpoint.key.clone()]),
            )
            .unwrap();
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        assert!(store.recover().unwrap().records.is_empty());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(path.clone(), window, cx);
            });
        });
        replace_document(&workspace, 0, "discarded text\n", cx);
        let discarded_checkpoint = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_at(0)
                .unwrap()
                .read(app)
                .recovery_checkpoint(app)
        });
        store
            .checkpoint(
                &discarded_checkpoint,
                &HashSet::from([discarded_checkpoint.key.clone()]),
            )
            .unwrap();
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn save_and_discard_supersede_an_in_flight_checkpoint(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in-flight.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        for decision in ["Save", "Discard"] {
            if workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()) {
                cx.update(|window, app| {
                    workspace.update(app, |workspace, cx| {
                        workspace.open_file(path.clone(), window, cx);
                    });
                });
            }
            replace_document(&workspace, 0, &format!("{decision} text\n"), cx);
            let (id, checkpoint, attempt) = workspace.read_with(cx, |workspace, app| {
                let document = workspace.document_at(0).unwrap().read(app);
                let state = workspace
                    .recovery_flow
                    .recovery_schedules
                    .get(&document.id())
                    .unwrap();
                let attempt = test_recovery_attempt(
                    state
                        .token
                        .clone()
                        .expect("a ready test store must provide a recovery token"),
                    state.content_identity.revision,
                    Instant::now(),
                    Arc::new(AtomicBool::new(false)),
                );
                (document.id(), document.recovery_checkpoint(app), attempt)
            });
            store
                .checkpoint(&checkpoint, &HashSet::from([checkpoint.key.clone()]))
                .unwrap();
            workspace.update(cx, |workspace, _| {
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .get_mut(&id)
                    .unwrap()
                    .in_flight = Some(attempt.clone());
            });

            cx.simulate_keystrokes("ctrl-w");
            cx.simulate_prompt_answer(decision);
            cx.run_until_parked();

            let outcome = store
                .checkpoint_if_current(
                    &checkpoint,
                    &HashSet::from([checkpoint.key.clone()]),
                    attempt.token.clone(),
                )
                .unwrap();
            assert!(matches!(outcome, CheckpointOutcome::Superseded));
            workspace.update(cx, |workspace, cx| {
                workspace.finish_recovery_checkpoints(
                    vec![(id, attempt, CheckpointBatchOutcome::Superseded)],
                    RecoveryMaintenance::default(),
                    cx.background_executor().now(),
                    cx,
                );
            });
            assert!(store.recover().unwrap().records.is_empty());
        }
    }

    #[gpui_kit::test]
    fn clean_and_closed_documents_retire_recovery_deadlines(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deadline.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store);
        replace_document(&workspace, 0, "save me\n", cx);
        let generation_before_save = workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_flow.recovery_schedules.len(), 1);
            assert!(workspace.recovery_flow._recovery_timer.is_some());
            workspace.recovery_flow.recovery_timer_generation
        });
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_schedules.is_empty());
            assert!(workspace.recovery_flow._recovery_timer.is_none());
            assert!(workspace.recovery_flow.recovery_timer_generation > generation_before_save);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(path, window, cx);
            });
        });
        replace_document(&workspace, 0, "discard me\n", cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_flow.recovery_schedules.len(), 1);
            assert!(workspace.recovery_flow._recovery_timer.is_some());
        });
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_schedules.is_empty());
            assert!(workspace.recovery_flow._recovery_timer.is_none());
        });
    }

    #[gpui_kit::test]
    fn stale_recovery_completion_cannot_clear_a_newer_attempt(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-completion.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let (id, key) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (document.id(), document.recovery_key())
        });
        let old_token = store.current_token(&key);
        store.invalidate_and_delete(&key).unwrap();
        let new_token = store.current_token(&key);
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let timing = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        let attempt = super::RecoveryAttempt {
            token: new_token.clone(),
            content_identity: super::RecoveryContentIdentity::for_revision(7),
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let deadline = schedule.next_deadline();

        workspace.update(cx, |workspace, cx| {
            workspace.recovery_flow.recovery_schedules.insert(
                id,
                super::DocumentRecoveryState {
                    key,
                    content_identity: super::RecoveryContentIdentity::for_revision(7),
                    suppressed_oversized_revision: None,
                    token: Some(new_token),
                    schedule,
                    in_flight: Some(attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.finish_recovery_checkpoints(
                vec![(
                    id,
                    test_recovery_attempt(old_token, 7, now, Arc::new(AtomicBool::new(false))),
                    CheckpointBatchOutcome::Written,
                )],
                RecoveryMaintenance::default(),
                cx.background_executor().now(),
                cx,
            );
        });

        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert_eq!(state.schedule.next_deadline(), deadline);
        });
    }

    #[gpui_kit::test]
    fn editing_one_document_does_not_cancel_other_recovery_attempts(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first_path = dir.path().join("first-cancelled.md");
        let second_path = dir.path().join("second-cancelled.md");
        fs::write(&first_path, "first\n").unwrap();
        fs::write(&second_path, "second\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first_path), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second_path, window, cx);
            });
        });
        replace_document(&workspace, 0, "first dirty\n", cx);
        replace_document(&workspace, 1, "second dirty\n", cx);

        let now = cx.background_executor.now();
        let first_cancelled = Arc::new(AtomicBool::new(false));
        let second_cancelled = Arc::new(AtomicBool::new(false));
        let (first_id, second_id) = workspace.read_with(cx, |workspace, app| {
            (
                workspace.document_at(0).unwrap().read(app).id(),
                workspace.document_at(1).unwrap().read(app).id(),
            )
        });
        workspace.update(cx, |workspace, app| {
            for id in [first_id, second_id] {
                let document = workspace.document_by_id(id, app).unwrap();
                let document = document.read(app);
                let mut schedule = CheckpointSchedule::default();
                schedule.mark_dirty(now);
                let attempt = RecoveryAttempt {
                    token: store.activate_and_current_token(&document.recovery_key()).0,
                    content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                    timing: schedule.checkpoint_dispatched(now).unwrap(),
                    cancelled: if id == first_id {
                        first_cancelled.clone()
                    } else {
                        second_cancelled.clone()
                    },
                };
                workspace.recovery_flow.recovery_schedules.insert(
                    id,
                    DocumentRecoveryState {
                        key: document.recovery_key(),
                        content_identity: RecoveryContentIdentity::for_revision(
                            document.revision(),
                        ),
                        suppressed_oversized_revision: None,
                        token: Some(attempt.token.clone()),
                        schedule,
                        in_flight: Some(attempt),
                        deadline_reported: false,
                        protection_warning: false,
                    },
                );
            }
        });

        replace_document(&workspace, 0, "newer first text\n", cx);

        assert!(first_cancelled.load(Ordering::Acquire));
        assert!(!second_cancelled.load(Ordering::Acquire));
        workspace.read_with(cx, |workspace, _| {
            let first = workspace
                .recovery_flow
                .recovery_schedules
                .get(&first_id)
                .unwrap();
            let second = workspace
                .recovery_flow
                .recovery_schedules
                .get(&second_id)
                .unwrap();
            assert!(first.in_flight.is_some());
            assert!(second.in_flight.is_some());
            assert!(!Arc::ptr_eq(
                &first.in_flight.as_ref().unwrap().cancelled,
                &second.in_flight.as_ref().unwrap().cancelled,
            ));
        });
    }

    #[gpui_kit::test]
    fn cancelled_checkpoint_catches_up_after_the_retry_throttle(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancelled-catch-up.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        replace_document(&workspace, 0, "first snapshot\n", cx);

        let now = cx.background_executor.now();
        let (id, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(now);
            (
                document.id(),
                RecoveryAttempt {
                    token: state
                        .token
                        .clone()
                        .expect("a ready test store must provide a recovery token"),
                    content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                    timing: schedule.checkpoint_dispatched(now).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&id)
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(now);
            let timing = schedule.checkpoint_dispatched(now).unwrap();
            state.schedule = schedule;
            state.in_flight = Some(RecoveryAttempt {
                timing,
                ..attempt.clone()
            });
        });

        cx.background_executor.advance_clock(Duration::from_secs(3));
        let latest = "latest exact text 中文 \u{1f680}\n";
        replace_document(&workspace, 0, latest, cx);
        assert!(attempt.cancelled.load(Ordering::Acquire));

        let cancelled_attempt = workspace.read_with(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .unwrap()
                .in_flight
                .clone()
                .unwrap()
        });
        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_checkpoints(
                vec![(id, cancelled_attempt, CheckpointBatchOutcome::Superseded)],
                RecoveryMaintenance::default(),
                cx.background_executor().now(),
                cx,
            );
        });
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, latest);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_warning.is_none());
        });
    }

    #[gpui_kit::test]
    fn active_checkpoint_worker_coalesces_repeated_edits_into_one_latest_follow_up(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coalesced-worker.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let batches_before = store.checkpoint_batch_count_for_test();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text("first snapshot\n".into(), window, cx);
            });
        });
        let edited_at = cx.background_executor.now();
        let (id, checkpoint, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(edited_at);
            (
                document.id(),
                document.recovery_checkpoint(app),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                    timing: schedule
                        .checkpoint_dispatched(edited_at + Duration::from_secs(2))
                        .unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&id)
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(edited_at);
            let timing = schedule
                .checkpoint_dispatched(edited_at + Duration::from_secs(2))
                .unwrap();
            state.schedule = schedule;
            state.in_flight = Some(RecoveryAttempt {
                timing,
                ..attempt.clone()
            });
            state.deadline_reported = false;
            workspace.recovery_flow.recovery_checkpoint_worker_active = true;
            workspace.recovery_flow._recovery_timer = None;
        });

        let (worker_paused, release_worker) = store.pause_after_checkpoint_final_check_for_test();
        let worker_store = store.clone();
        let worker_checkpoint = checkpoint.clone();
        let worker_attempt = attempt.clone();
        let worker_key = checkpoint.key.clone();
        let worker = std::thread::spawn(move || {
            worker_store.checkpoint_batch_if_current_cancellable(
                [mt_core::recovery::CancellableRecoveryCheckpointAttempt {
                    checkpoint: &worker_checkpoint,
                    token: &worker_attempt.token,
                    cancelled: worker_attempt.cancelled.as_ref(),
                }],
                &HashSet::from([worker_key]),
            )
        });
        worker_paused
            .recv_timeout(Duration::from_secs(1))
            .expect("the first physical checkpoint batch must reach the publish boundary");

        cx.background_executor.advance_clock(
            attempt
                .timing
                .durable_complete_by
                .saturating_duration_since(cx.background_executor.now()),
        );
        workspace.update(cx, |workspace, cx| workspace.checkpoint_recovery(cx));
        let mut latest = String::new();
        for revision in 1..=4 {
            latest = format!("latest revision {revision} 中文 \u{1f680}\n");
            cx.update(|window, app| {
                document.update(app, |document, cx| {
                    document.replace_text(latest.clone(), window, cx);
                });
            });
        }
        cx.background_executor.advance_clock(Duration::from_secs(1));
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery(cx);
        });
        let same_attempt_while_paused = workspace.read_with(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .and_then(|state| state.in_flight.as_ref())
                .is_some_and(|current| current == &attempt)
        });
        let batches_while_paused = store.checkpoint_batch_count_for_test();

        release_worker.send(()).unwrap();
        let batch = worker.join().unwrap();
        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_checkpoints(
                vec![(id, attempt, batch.outcomes.into_iter().next().unwrap())],
                batch.maintenance,
                cx.background_executor().now(),
                cx,
            );
        });
        cx.run_until_parked();
        let recovered = store.recover().unwrap();

        assert!(
            same_attempt_while_paused,
            "an occupied worker slot must retain the cancelled logical attempt instead of snapshotting again"
        );
        assert_eq!(
            batches_while_paused,
            batches_before + 1,
            "deadline handling must not start a second physical batch"
        );
        assert_eq!(
            store.checkpoint_batch_count_for_test(),
            batches_before + 2,
            "the released slot must dispatch exactly one coalesced follow-up"
        );
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, latest);
    }

    #[gpui_kit::test]
    fn checkpoint_returning_after_its_durable_deadline_keeps_the_warning(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store-late.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        replace_document(&workspace, 0, "late checkpoint\n", cx);
        let dispatched_at = cx.background_executor.now();
        let (id, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            (
                document.id(),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                    timing: schedule.checkpoint_dispatched(dispatched_at).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&id)
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            let _ = schedule.checkpoint_dispatched(dispatched_at).unwrap();
            state.schedule = schedule;
            state.in_flight = Some(attempt.clone());
            state.protection_warning = false;
        });
        cx.background_executor.advance_clock(Duration::from_secs(9));

        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_checkpoints(
                vec![(id, attempt, CheckpointBatchOutcome::Written)],
                RecoveryMaintenance::default(),
                cx.background_executor().now(),
                cx,
            );
        });

        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(state.schedule.next_deadline().is_some());
            assert!(workspace.recovery_flow.recovery_warning.is_some());
        });
    }

    #[gpui_kit::test]
    fn checkpoint_returning_on_time_clears_a_warning_even_when_ui_delivery_is_late(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-late.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        replace_document(&workspace, 0, "durable on time\n", cx);
        let dispatched_at = cx.background_executor.now();
        let (id, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            (
                document.id(),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    content_identity: RecoveryContentIdentity::for_revision(document.revision()),
                    timing: schedule.checkpoint_dispatched(dispatched_at).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&id)
                .unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            let _ = schedule.checkpoint_dispatched(dispatched_at).unwrap();
            state.schedule = schedule;
            state.in_flight = Some(attempt.clone());
            state.protection_warning = false;
        });

        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(attempt.timing.durable_complete_by, cx);
        });
        let store_returned_at = attempt
            .timing
            .durable_complete_by
            .checked_sub(Duration::from_secs(1))
            .unwrap();
        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_checkpoints(
                vec![(id, attempt, CheckpointBatchOutcome::Written)],
                RecoveryMaintenance::default(),
                store_returned_at,
                cx,
            );
        });

        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert!(!state.protection_warning);
            assert!(workspace.recovery_flow.recovery_warning.is_none());
        });
    }

    #[gpui_kit::test]
    fn overdue_checkpoint_waits_for_the_physical_worker_before_retrying(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overdue-stuck.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let latest = "latest while stale worker is stuck\n";
        replace_document(&workspace, 0, latest, cx);

        let now = cx.background_executor.now();
        let (id, token, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            (
                document.id(),
                state
                    .token
                    .clone()
                    .expect("a ready test store must provide a recovery token"),
                document.revision(),
            )
        });
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = RecoveryAttempt {
            token,
            content_identity: RecoveryContentIdentity::for_revision(revision),
            timing: schedule.checkpoint_dispatched(now).unwrap(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, _| {
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get_mut(&id)
                .unwrap();
            state.schedule = schedule;
            state.in_flight = Some(attempt.clone());
            state.deadline_reported = false;
            workspace.recovery_flow.recovery_checkpoint_worker_active = true;
        });

        cx.background_executor.advance_clock(
            attempt
                .timing
                .durable_complete_by
                .saturating_duration_since(now),
        );
        let deadline = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(deadline, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(state.deadline_reported);
            assert!(state.protection_warning);
            assert!(workspace.recovery_flow._recovery_timer.is_none());
        });
        assert!(attempt.cancelled.load(Ordering::Acquire));

        cx.background_executor.advance_clock(Duration::from_secs(1));
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(workspace.recovery_flow.recovery_warning.is_some());
        });
        assert!(store.recover().unwrap().records.is_empty());
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery_checkpoint_worker_active = false;
            workspace.recovery_flow.recovery_schedules.remove(&id);
            workspace.recovery_flow._recovery_timer = None;
        });
    }

    #[gpui_kit::test]
    fn overdue_checkpoint_retries_and_clears_warning_after_becoming_durable(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overdue.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        replace_document(&workspace, 0, "protected late\n", cx);

        let now = cx.background_executor.now();
        let (id, key, revision, token) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            (
                document.id(),
                document.recovery_key(),
                document.revision(),
                state
                    .token
                    .clone()
                    .expect("a ready test store must provide a recovery token"),
            )
        });
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let timing = schedule.checkpoint_dispatched(now).unwrap();
        let attempt = RecoveryAttempt {
            token: token.clone(),
            content_identity: RecoveryContentIdentity::for_revision(revision),
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery_schedules.insert(
                id,
                DocumentRecoveryState {
                    key,
                    content_identity: RecoveryContentIdentity::for_revision(revision),
                    suppressed_oversized_revision: None,
                    token: Some(token),
                    schedule,
                    in_flight: Some(attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.recovery_flow.recovery_checkpoint_worker_active = true;
        });

        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(timing.durable_complete_by, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(state.deadline_reported);
            assert!(state.protection_warning);
            assert!(workspace.recovery_flow.recovery_warning.is_some());
            assert!(workspace.recovery_flow._recovery_timer.is_none());
        });
        assert!(attempt.cancelled.load(Ordering::Acquire));

        let overdue_attempt = attempt;
        cx.background_executor
            .advance_clock(timing.durable_complete_by.saturating_duration_since(now));
        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_checkpoints(
                vec![(
                    id,
                    overdue_attempt,
                    CheckpointBatchOutcome::Failed(RecoveryError::Protection),
                )],
                RecoveryMaintenance::default(),
                cx.background_executor().now(),
                cx,
            );
            workspace.recovery_flow._recovery_timer = None;
        });

        let latest = "latest after overdue 中文 \u{1f680}\n";
        replace_document(&workspace, 0, latest, cx);
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow._recovery_timer = None;
        });
        cx.background_executor.advance_clock(Duration::from_secs(1));
        let retry_at = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.recovery_flow._recovery_timer = None;
            workspace.checkpoint_recovery_at(retry_at, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let retry = workspace
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .unwrap()
                .in_flight
                .as_ref()
                .unwrap();
            assert_eq!(
                retry.timing.durable_complete_by,
                retry_at + Duration::from_secs(8)
            );
            assert!(workspace.recovery_flow.recovery_warning.is_some());
        });
        cx.run_until_parked();

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, latest);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_warning.is_none());
        });
    }

    #[gpui_kit::test]
    fn obvious_oversized_revision_does_no_physical_work_until_a_smaller_edit(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("obvious-oversized.md");
        fs::write(&path, "disk\n").unwrap();
        let protector = Arc::new(CountingRecoveryProtector::new(
            CountingProtection::Reversible,
        ));
        let max_record_bytes = 4 * 1024;
        let store = RecoveryStore::new_at_with_limits(
            dir.path().join("recovery-store"),
            protector.clone(),
            recovery_limits(max_record_bytes),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());

        replace_document(
            &workspace,
            0,
            &"x".repeat(max_record_bytes as usize + 1),
            cx,
        );
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        for elapsed in [Duration::from_secs(1), Duration::from_secs(30)] {
            cx.background_executor.advance_clock(elapsed);
            cx.run_until_parked();
        }

        assert_eq!(store.checkpoint_batch_count_for_test(), 0);
        assert_eq!(protector.calls(), 0);
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.recovery_flow.recovery_checkpoint_worker_active);
            assert!(workspace.recovery_flow.recovery_warning.is_some());
            assert!(workspace.recovery_flow._recovery_timer.is_none());
        });

        let smaller = "small recoverable edit 中文 \u{1f680}\n";
        replace_document(&workspace, 0, smaller, cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        assert_eq!(store.checkpoint_batch_count_for_test(), 1);
        assert_eq!(protector.calls(), 1);
        assert_eq!(store.recover().unwrap().records[0].record.text, smaller);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_warning.is_none());
        });
    }

    #[gpui_kit::test]
    fn ciphertext_oversize_is_not_retried_until_the_document_is_edited(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ciphertext-oversized.md");
        fs::write(&path, "disk\n").unwrap();
        let max_record_bytes = 4 * 1024;
        let protector = Arc::new(CountingRecoveryProtector::new(CountingProtection::Expand(
            max_record_bytes as usize,
        )));
        let store = RecoveryStore::new_at_with_limits(
            dir.path().join("recovery-store"),
            protector.clone(),
            recovery_limits(max_record_bytes),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        replace_document(&workspace, 0, "below the plaintext ceiling\n", cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 1);
        assert_eq!(store.checkpoint_batch_count_for_test(), 1);
        assert!(workspace.read_with(cx, |workspace, _| {
            workspace.recovery_flow.recovery_warning.is_some()
        }));

        cx.background_executor
            .advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 1);
        assert_eq!(store.checkpoint_batch_count_for_test(), 1);

        replace_document(&workspace, 0, "a different below-ceiling revision\n", cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 2);
        assert_eq!(store.checkpoint_batch_count_for_test(), 2);
    }

    #[gpui_kit::test]
    fn transient_protection_failure_retries_the_same_revision_and_succeeds(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transient-protection.md");
        fs::write(&path, "disk\n").unwrap();
        let protector = Arc::new(CountingRecoveryProtector::new(CountingProtection::FailOnce));
        let store =
            RecoveryStore::new_at(dir.path().join("recovery-store"), protector.clone()).unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let text = "retry this exact revision 中文 \u{1f680}\n";
        replace_document(&workspace, 0, text, cx);
        let revision = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).revision()
        });
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 1);
        assert!(workspace.read_with(cx, |workspace, _| {
            workspace.recovery_flow.recovery_warning.is_some()
        }));

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert_eq!(protector.calls(), 2);
        assert_eq!(store.checkpoint_batch_count_for_test(), 2);
        assert_eq!(store.recover().unwrap().records[0].record.text, text);
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(
                workspace.document_at(0).unwrap().read(app).revision(),
                revision
            );
            assert!(workspace.recovery_flow.recovery_warning.is_none());
        });
    }

    #[gpui_kit::test]
    fn stale_written_checkpoint_does_not_clear_an_existing_recovery_warning(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-written-warning.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        replace_document(&workspace, 0, "current revision\n", cx);

        let now = cx.background_executor.now();
        let (id, key, revision, token) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            (
                document.id(),
                document.recovery_key(),
                document.revision(),
                state
                    .token
                    .clone()
                    .expect("a ready test store must provide a recovery token"),
            )
        });
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let timing = schedule.checkpoint_dispatched(now).unwrap();
        schedule.mark_dirty(now + Duration::from_secs(1));
        let stale_binding =
            RevisionRequestBinding::new([1; 32], revision, 0, [2; 32], [3; 32], [4; 32]);
        let current_binding =
            RevisionRequestBinding::new([1; 32], revision, 0, [2; 32], [3; 32], [5; 32]);
        let attempt = RecoveryAttempt {
            token: token.clone(),
            content_identity: RecoveryContentIdentity {
                revision,
                revision_binding: Some(stale_binding),
            },
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, cx| {
            workspace.recovery_flow.recovery_schedules.insert(
                id,
                DocumentRecoveryState {
                    key,
                    content_identity: RecoveryContentIdentity {
                        revision,
                        revision_binding: Some(current_binding),
                    },
                    suppressed_oversized_revision: None,
                    token: Some(token),
                    schedule,
                    in_flight: Some(attempt.clone()),
                    deadline_reported: false,
                    protection_warning: true,
                },
            );
            workspace.finish_recovery_checkpoints(
                vec![(id, attempt, CheckpointBatchOutcome::Written)],
                RecoveryMaintenance::default(),
                cx.background_executor().now(),
                cx,
            );
        });

        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(workspace.recovery_flow.recovery_warning.is_some());
        });
    }

    #[gpui_kit::test]
    fn eviction_reservation_warns_until_a_later_checkpoint_is_written(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reserved-document.md");
        let incoming_path = dir.path().join("incoming-document.md");
        fs::write(&path, "disk\n").unwrap();
        fs::write(&incoming_path, "incoming disk\n").unwrap();
        let store = RecoveryStore::new_at_with_limits(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
            RecoveryLimits {
                max_records: 1,
                max_record_bytes: 10_000,
                max_total_bytes: 20_000,
                max_age: Duration::from_secs(7 * 24 * 60 * 60),
            },
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "older recovery text\n");
        let incoming = RecoveryCheckpoint {
            key: RecoveryKey::for_path(&incoming_path),
            text: "incoming recovery text\n".into(),
            metadata: RecoveryMetadata::from_loaded_file(
                &mt_core::document::io::load(&incoming_path).unwrap(),
            ),
            revision: None,
        };
        let token = store.current_token(&incoming.key);
        let (reserved, release) = store.pause_after_eviction_reservation_for_test();
        let worker_store = store.clone();
        let worker = std::thread::spawn(move || {
            worker_store
                .checkpoint_if_current(&incoming, &HashSet::new(), token)
                .unwrap()
        });
        reserved
            .recv_timeout(Duration::from_secs(1))
            .expect("the transaction must reserve its eviction victim");

        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let edited = "edited during eviction reservation\n";
        replace_document(&workspace, 0, edited, cx);
        let immediate_warning = workspace.read_with(cx, |workspace, _| {
            workspace.recovery_flow.recovery_warning.clone()
        });

        release.send(()).unwrap();
        assert!(matches!(
            worker.join().unwrap(),
            CheckpointOutcome::Written(_)
        ));
        let warning = "Recovery protection is unavailable for at least one dirty document. Editing and source files are unchanged.";
        assert_eq!(immediate_warning.as_deref(), Some(warning));
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.recovery_flow.recovery_warning.as_deref(),
                Some(warning)
            );
        });

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_warning.is_none());
            assert!(
                workspace
                    .recovery_flow
                    .recovery_schedules
                    .values()
                    .all(|state| !state.protection_warning)
            );
        });
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, edited);
    }

    #[gpui_kit::test]
    fn continued_edits_checkpoint_without_postponing_the_oldest_uncovered_text(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("continuous-checkpoint.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let first = "first checkpoint\n";
        replace_document(&workspace, 0, first, cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let first_scan = store.recover().unwrap();
        assert_eq!(first_scan.records.len(), 1);
        assert_eq!(first_scan.records[0].record.text, first);

        let mut latest = first.to_string();
        let mut latest_before_last_edit = first.to_string();
        for second in 1..=9 {
            cx.background_executor.advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            latest_before_last_edit.clone_from(&latest);
            latest = format!("latest revision {second} 中文 \\u{{1f680}}\n");
            replace_document(&workspace, 0, &latest, cx);
        }

        let before_deadline = store.recover().unwrap();
        assert_eq!(before_deadline.records.len(), 1);
        assert_eq!(
            before_deadline.records[0].record.text,
            latest_before_last_edit
        );

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let after_deadline = store.recover().unwrap();
        assert_eq!(after_deadline.records.len(), 1);
        assert_eq!(after_deadline.records[0].record.text, latest);
    }

    #[gpui_kit::test]
    fn simultaneously_due_documents_share_one_recovery_scan(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.md");
        let second = dir.path().join("second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });

        replace_document(&workspace, 0, "first exact 中文 \u{1f680}\n", cx);
        replace_document(&workspace, 1, "second exact 中文 \u{1f680}\n", cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        assert_eq!(
            store.retention_scan_count_for_test(),
            1,
            "one scheduler wake-up must scan retention once for the whole due batch"
        );
        let recovered: std::collections::HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|record| (record.record.key, record.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert!(
            recovered
                .values()
                .any(|text| text == "first exact 中文 \u{1f680}\n")
        );
        assert!(
            recovered
                .values()
                .any(|text| text == "second exact 中文 \u{1f680}\n")
        );
    }

    #[gpui_kit::test]
    fn recovery_idle_deadline_restores_exact_cjk_and_emoji_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path), store.clone());
        let edited = "checkpoint 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        let started = Instant::now();
        workspace.update(cx, |workspace, cx| {
            let document = workspace.document_at(0).cloned().unwrap();
            workspace.arm_document_recovery_at(&document, started, cx);
            workspace.checkpoint_recovery_at(started + Duration::from_secs(1), cx);
        });
        assert!(
            store.recover().unwrap().records.is_empty(),
            "the idle checkpoint must not fire before two seconds"
        );

        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(started + Duration::from_secs(2), cx);
        });
        cx.run_until_parked();
        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1, "the two-second deadline dispatched");
        assert_eq!(scan.records[0].record.text, edited);

        let (restored_workspace, cx) =
            open_test_workspace_with_recovery_store(cx, None, store.clone());
        let restored = cx.update(|window, app| {
            restored_workspace.update(app, |workspace, cx| {
                restore_recovery_for_test(workspace, scan, window, cx)
            })
        });
        assert_eq!(restored, (1, 0));
        let restored_document = restored_workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        restored_document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert_eq!(document.text(app), edited);
        });
    }

    #[gpui_kit::test]
    fn no_record_startup_recovery_clears_pending_state(cx: &mut TestAppContext) {
        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, None, StartupRecovery::default);
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.recovery_flow.startup_recovery_pending);
            assert!(workspace.recovery_flow.recovery.is_none());
            assert!(workspace.tabs.is_empty());
        });
    }

    #[gpui_kit::test]
    fn early_edit_keeps_its_deadline_until_startup_recovery_is_available(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup-pending.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
        });

        let edited_at = cx.background_executor.now();
        replace_document(&workspace, 0, "typed while recovery opens\n", cx);
        let id = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .expect("an early edit must create recovery timing state");
            assert_eq!(
                state.schedule.next_deadline(),
                Some(edited_at + Duration::from_secs(2))
            );
            document.id()
        });

        let unavailable_at = edited_at + Duration::from_secs(2);
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(unavailable_at, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(workspace.recovery_flow.recovery_warning.is_some());
        });

        let store_ready_at = edited_at + Duration::from_secs(3);
        complete_startup_with_store(&workspace, store, cx);
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert!(
                state.token.is_some(),
                "the startup result must activate an existing unprotected schedule"
            );
            assert_eq!(
                state.schedule.next_deadline(),
                Some(edited_at + Duration::from_secs(2)),
                "activating recovery must preserve the first edit's deadline"
            );
            assert!(
                workspace.recovery_flow._recovery_timer.is_some(),
                "the overdue schedule must be re-armed when recovery becomes available"
            );
        });
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(store_ready_at, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let attempt = workspace
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .and_then(|state| state.in_flight.as_ref())
                .expect("the overdue early edit must dispatch when recovery becomes available");
            assert_eq!(
                attempt.timing.durable_complete_by,
                edited_at + Duration::from_secs(10)
            );
        });
    }

    #[gpui_kit::test]
    fn populated_startup_recovery_does_not_block_initial_file_or_early_edits(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("initial.md");
        fs::write(&path, "disk\n").unwrap();
        let recovered_text = "older recovered text\n";
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);
        let recovered_key = RecoveryKey::for_path(&path);
        let store_for_startup = store.clone();
        let initial_was_interactive = Arc::new(AtomicBool::new(false));
        let loader_observation = initial_was_interactive.clone();
        let inspection_observation = initial_was_interactive.clone();
        let latest = "typed before recovery 中文 \u{1f680}\n";

        let (workspace, cx) = open_test_workspace_with_startup_recovery_inspection(
            cx,
            Some(path.clone()),
            move || {
                assert!(
                    loader_observation.load(Ordering::Acquire),
                    "the initial file must be open and editable before recovery loading begins"
                );
                populated_startup_recovery(store_for_startup)
            },
            move |workspace, window, cx| {
                assert_eq!(workspace.tabs.len(), 1);
                let document = workspace.document_at(0).cloned().unwrap();
                assert_eq!(document.read(cx).text(cx), "disk\n");
                document.update(cx, |document, cx| {
                    document.replace_text(latest.to_string(), window, cx);
                });
                assert_eq!(document.read(cx).text(cx), latest);
                inspection_observation.store(true, Ordering::Release);
            },
        );

        cx.run_until_parked();

        let id = workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let live = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(open_path)
                        if open_path == &path =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the initial file must remain open");
            let live = live.read(app);
            assert!(live.is_dirty());
            assert_eq!(live.text(app), latest);
            live.id()
        });
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.recovery_flow.recovery.is_some());
            assert!(workspace.recovery_flow.recovery_schedules.contains_key(&id));
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Restored 1 recovery checkpoint(s); skipped 0 unavailable or invalid record(s)."
                )
            );
            let recovered = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::Recovered(key)
                        if key == &recovered_key =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the old checkpoint must be visible separately");
            let recovered = recovered.read(app);
            assert!(recovered.is_dirty());
            assert_eq!(recovered.text(app), recovered_text);
        });
    }

    #[gpui_kit::test]
    fn explicit_reload_during_startup_keeps_old_checkpoint(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reloaded-before-scan.md");
        let disk_text = "clean disk A\n";
        let old_checkpoint = "old unsaved A checkpoint\n";
        fs::write(&path, disk_text).unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, old_checkpoint);

        let reloaded_before_scan = Arc::new(AtomicBool::new(false));
        let scan_after_reload = reloaded_before_scan.clone();
        let store_for_startup = store.clone();
        let reload_observation = reloaded_before_scan.clone();
        let (workspace, cx) = open_test_workspace_with_startup_recovery_inspection(
            cx,
            Some(path.clone()),
            move || {
                assert!(
                    scan_after_reload.load(Ordering::Acquire),
                    "the deferred startup scan must run after explicit Reload"
                );
                populated_startup_recovery(store_for_startup)
            },
            move |workspace, window, cx| {
                let document = workspace.document_at(0).cloned().unwrap();
                let before = document.read(cx).source_stamp();
                document.update(cx, |document, cx| document.reload(window, cx));
                let after = document.read(cx).source_stamp();
                assert_ne!(before, after, "explicit Reload advances source identity");
                document.read_with(cx, |document, app| {
                    assert_eq!(document.text(app), disk_text);
                    assert!(!document.is_dirty());
                });
                reload_observation.store(true, Ordering::Release);
            },
        );
        cx.run_until_parked();

        let live_document = workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let live = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(open_path)
                        if open_path == &path =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the reloaded A document must remain open");
            let live_read = live.read(app);
            assert_eq!(live_read.text(app), disk_text);
            assert!(!live_read.is_dirty());
            assert!(!live_read.is_externally_changed());
            assert_eq!(live_read.source_path(), Some(path.as_path()));
            assert_ne!(live_read.recovery_key(), RecoveryKey::for_path(&path));
            live.clone()
        });
        workspace.read_with(cx, |workspace, app| {
            let recovered = workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::Recovered(key)
                        if key == &RecoveryKey::for_path(&path) =>
                    {
                        Some(tab.payload.view.clone())
                    }
                    _ => None,
                })
                .expect("the old A checkpoint must be exposed as a recovered-only tab");
            let recovered = recovered.read(app);
            assert_eq!(recovered.text(app), old_checkpoint);
            assert!(recovered.is_dirty());
        });

        let live_text = "edited after explicit Reload\n";
        replace_document(&workspace, 0, live_text, cx);
        let live_key = live_document.read_with(cx, |document, _| document.recovery_key());
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 2);
        let old_record = records
            .iter()
            .find(|record| record.record.key == RecoveryKey::for_path(&path))
            .expect("the old path-key checkpoint must remain durable");
        assert_eq!(old_record.record.text, old_checkpoint);
        let live_record = records
            .iter()
            .find(|record| record.record.key == live_key)
            .expect("the reloaded file's edit must use its own recovery key");
        assert_eq!(live_record.record.text, live_text);
        assert_ne!(live_record.record.key, old_record.record.key);
        assert_eq!(fs::read_to_string(&path).unwrap(), disk_text);
    }

    #[gpui_kit::test]
    fn recovered_tab_path_actions_use_the_document_source_path(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovered-path.md");
        fs::write(&path, "disk source\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "recovered source\n");

        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let live_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        live_document.update(cx, |document, _| {
            document.rotate_recovery_key();
        });
        let scan = store.recover().unwrap();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert_eq!(
                    restore_recovery_for_test(workspace, scan, window, cx),
                    (1, 0)
                );
            });
        });
        cx.run_until_parked();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_folder(dir.path().to_path_buf(), window, cx);
            });
        });

        let recovered_index = workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let recovered = workspace.tabs.active().unwrap();
            assert!(recovered.path().is_none());
            assert!(matches!(
                &recovered.identity,
                mt_core::workspace::tabs::TabIdentity::Recovered(key)
                    if key == &RecoveryKey::for_path(&path)
            ));
            assert_eq!(
                recovered.payload.view.read(app).source_path(),
                Some(path.as_path())
            );
            workspace.tabs.active_index()
        });
        let expected_path = path.to_string_lossy().replace('\\', "/");

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_copy_path(&super::CopyPath, window, cx);
            });
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected_path.clone())
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.tabs.set_menu(recovered_index);
                workspace.on_copy_path(&super::CopyPath, window, cx);
            });
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected_path)
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.tabs.set_menu(recovered_index);
                workspace.on_copy_relative_path(&super::CopyRelativePath, window, cx);
            });
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("recovered-path.md".to_owned())
        );
    }

    #[gpui_kit::test]
    fn recovered_search_reveal_targets_the_buffer_and_stale_result_is_inert(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search-recovered-b.md");
        let control_path = dir.path().join("search-control-c.md");
        let ordinary_text = "ordinary B has needle\n";
        let recovered_text = "recovered B has needle\n";
        let control_text = "control C remains active\n";
        fs::write(&path, ordinary_text).unwrap();
        fs::write(&control_path, control_text).unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);

        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let ordinary = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let ordinary_id = ordinary.read_with(cx, |document, _| document.id());
        ordinary.update(cx, |document, _| document.rotate_recovery_key());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(control_path.clone(), window, cx);
            });
        });
        let control_id = workspace.read_with(cx, |workspace, app| {
            workspace
                .tabs
                .iter()
                .find_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::File(open_path)
                        if open_path == &control_path =>
                    {
                        Some(tab.payload.view.read(app).id())
                    }
                    _ => None,
                })
                .unwrap()
        });
        let recovered_key = RecoveryKey::for_path(&path);
        let scan = store.recover().unwrap();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert_eq!(
                    restore_recovery_for_test(workspace, scan, window, cx),
                    (1, 0)
                );
            });
        });
        cx.run_until_parked();

        let (recovered_id, recovered_index, control_index) =
            workspace.read_with(cx, |workspace, app| {
                let (recovered_index, recovered) = workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .find(|(_, tab)| {
                        matches!(
                            &tab.identity,
                            mt_core::workspace::tabs::TabIdentity::Recovered(key)
                                if key == &recovered_key
                        )
                    })
                    .expect("the checkpoint must appear as a recovered-only tab");
                let recovered = recovered.payload.view.read(app);
                assert_eq!(recovered.source_path(), Some(path.as_path()));
                assert_eq!(recovered.text(app), recovered_text);
                assert_ne!(recovered.id(), ordinary_id);
                let control_index = workspace
                    .tabs
                    .iter()
                    .position(|tab| {
                        matches!(
                            &tab.identity,
                            mt_core::workspace::tabs::TabIdentity::File(open_path)
                                if open_path == &control_path
                        )
                    })
                    .unwrap();
                (recovered.id(), recovered_index, control_index)
            });
        assert_eq!(
            ordinary.read_with(cx, |document, app| document.text(app)),
            ordinary_text
        );

        let mut matches = Results::default();
        search_open_document(
            recovered_id,
            &path,
            recovered_text,
            &Query::new("needle"),
            mt_core::workspace::search::DEFAULT_LIMIT,
            &mut matches,
        );
        assert_eq!(matches.matches.len(), 1);
        let hit = &matches.matches[0];
        assert_eq!(hit.target, SearchTarget::OpenDocument(recovered_id));
        assert_eq!(
            &recovered_text[hit.offset..hit.offset + "needle".len()],
            "needle"
        );
        let reveal = SearchEvent::Reveal {
            path: hit.path.as_ref().clone(),
            target: hit.target,
            offset: hit.offset,
        };

        emit_search_reveal(&workspace, reveal.clone(), cx);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 3);
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), recovered_id);
            assert_eq!(active.source_path(), Some(path.as_path()));
            assert_eq!(active.text(app), recovered_text);
            assert_eq!(active.cursor(app), hit.offset);
            let ordinary = workspace
                .document_at(workspace.tabs.index_of(&path).unwrap())
                .unwrap()
                .read(app);
            assert_eq!(ordinary.id(), ordinary_id);
            assert_eq!(ordinary.text(app), ordinary_text);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.tabs.focus(control_index));
                workspace.request_close_tab(recovered_index, window, cx);
            });
        });
        assert!(
            cx.has_pending_prompt(),
            "closing the recovered buffer asks for a decision"
        );
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.tabs.index_of(&path).is_some());
            assert_eq!(
                workspace.active_document().unwrap().read(app).id(),
                control_id
            );
        });

        emit_search_reveal(&workspace, reveal, cx);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(
                workspace.tabs.len(),
                2,
                "a stale result must not recreate a tab"
            );
            assert!(workspace.tabs.index_of(&path).is_some());
            assert!(workspace.tabs.index_of(&control_path).is_some());
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(
                active.id(),
                control_id,
                "a stale result must not focus File(B)"
            );
            assert_eq!(active.text(app), control_text);
            let ordinary = workspace
                .document_at(workspace.tabs.index_of(&path).unwrap())
                .unwrap()
                .read(app);
            assert_eq!(ordinary.id(), ordinary_id);
            assert_eq!(ordinary.text(app), ordinary_text);
            assert!(workspace.tabs.iter().all(|tab| !matches!(
                &tab.identity,
                mt_core::workspace::tabs::TabIdentity::Recovered(key)
                    if key == &recovered_key
            )));
        });
    }

    #[gpui_kit::test]
    fn recovered_outline_jumps_do_not_reopen_the_ordinary_file(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outline-recovery.md");
        fs::write(&path, "# Ordinary file\n").unwrap();
        let recovered_text = "# Recovered first\n\n## Recovered second\n";
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (recovered_id, _) = restore_open_file_checkpoint(&workspace, &path, &store, cx);
        workspace.read_with(cx, |workspace, _| assert!(!workspace.history.can_go_back()));

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.reveal_offset(0, window, cx);
                workspace.reveal_offset(recovered_text.find("##").unwrap(), window, cx);
                workspace.on_navigate_back(&super::NavigateBack, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), recovered_id);
            assert_eq!(active.text(app), recovered_text);
            assert!(!workspace.history.can_go_back());
            assert!(!workspace.history.can_go_forward());
        });
    }

    #[gpui_kit::test]
    fn missing_file_search_result_cannot_move_recovered_buffer_or_add_history(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-search-result-b.md");
        let control_path = dir.path().join("history-control-c.md");
        let ordinary_text = "ordinary B result needle\n";
        let recovered_text = "recovered B has needle\n";
        fs::write(&path, ordinary_text).unwrap();
        fs::write(&control_path, "control C\n").unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);

        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(control_path.clone(), window, cx);
            });
        });
        let (recovered_id, _) = restore_open_file_checkpoint(&workspace, &path, &store, cx);

        let mut file_results = Results::default();
        search_files(
            std::slice::from_ref(&path),
            &Query::new("needle"),
            mt_core::workspace::search::DEFAULT_LIMIT,
            &mut file_results,
        );
        assert_eq!(file_results.matches.len(), 1);
        let file_hit = &file_results.matches[0];
        assert_eq!(file_hit.target, SearchTarget::File);
        let file_reveal = SearchEvent::Reveal {
            path: file_hit.path.as_ref().clone(),
            target: file_hit.target,
            offset: file_hit.offset,
        };

        let mut recovered_results = Results::default();
        search_open_document(
            recovered_id,
            &path,
            recovered_text,
            &Query::new("needle"),
            mt_core::workspace::search::DEFAULT_LIMIT,
            &mut recovered_results,
        );
        let recovered_hit = &recovered_results.matches[0];
        assert_eq!(
            recovered_hit.target,
            SearchTarget::OpenDocument(recovered_id)
        );
        assert_ne!(file_hit.offset, recovered_hit.offset);
        let recovered_reveal = SearchEvent::Reveal {
            path: recovered_hit.path.as_ref().clone(),
            target: recovered_hit.target,
            offset: recovered_hit.offset,
        };
        emit_search_reveal(&workspace, recovered_reveal, cx);
        cx.run_until_parked();

        let ordinary_index = workspace.read_with(cx, |workspace, _| {
            workspace
                .tabs
                .index_of(&path)
                .expect("ordinary B is still open")
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_close_tab(ordinary_index, window, cx);
            });
        });
        assert!(
            !cx.has_pending_prompt(),
            "the clean ordinary tab closes directly"
        );
        fs::remove_file(&path).unwrap();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.record_visit(control_path.clone(), 0);
                workspace.record_visit(control_path.clone(), 1);
                workspace.on_navigate_back(&super::NavigateBack, window, cx);
            });
        });
        let recovered_index = workspace.read_with(cx, |workspace, app| {
            workspace
                .tabs
                .iter()
                .enumerate()
                .find_map(|(index, tab)| {
                    (tab.payload.view.read(app).id() == recovered_id).then_some(index)
                })
                .expect("recovered B remains open")
        });
        workspace.update(cx, |workspace, _| {
            assert!(workspace.tabs.focus(recovered_index));
            assert!(workspace.history.can_go_forward());
        });
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.tabs.index_of(&path).is_none());
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), recovered_id);
        });
        let (cursor_before, tabs_before) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.active_document().unwrap().read(app);
            assert_eq!(document.id(), recovered_id);
            (document.cursor(app), workspace.tabs.len())
        });

        emit_search_reveal(&workspace, file_reveal, cx);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), tabs_before);
            assert!(workspace.tabs.index_of(&path).is_none());
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), recovered_id);
            assert_eq!(active.cursor(app), cursor_before);
            assert!(workspace.history.can_go_forward());
        });
    }

    #[gpui_kit::test]
    fn recovered_search_result_is_inert_after_save_as_changes_tab_identity(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save-as-source-b.md");
        let destination = dir.path().join("save-as-target-c.md");
        let recovered_text = "recovered B has needle\n";
        fs::write(&path, "ordinary B source\n").unwrap();

        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (recovered_id, _) = restore_open_file_checkpoint(&workspace, &path, &store, cx);

        let mut recovered_results = Results::default();
        search_open_document(
            recovered_id,
            &path,
            recovered_text,
            &Query::new("needle"),
            mt_core::workspace::search::DEFAULT_LIMIT,
            &mut recovered_results,
        );
        assert_eq!(recovered_results.matches.len(), 1);
        let hit = &recovered_results.matches[0];
        assert_eq!(hit.target, SearchTarget::OpenDocument(recovered_id));
        assert_eq!(hit.path.as_path(), path.as_path());
        assert!(hit.offset > 0);
        let stale_reveal = SearchEvent::Reveal {
            path: hit.path.as_ref().clone(),
            target: hit.target,
            offset: hit.offset,
        };

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    recovered_id,
                    destination.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        let destination_view = workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let index = workspace.tabs.index_of(&destination).unwrap();
            let tab = workspace.tabs.get(index).unwrap();
            assert!(matches!(
                &tab.identity,
                mt_core::workspace::tabs::TabIdentity::File(saved_path)
                    if saved_path == &destination
            ));
            let document = tab.payload.view.read(app);
            assert_eq!(document.id(), recovered_id);
            assert_eq!(document.source_path(), Some(destination.as_path()));
            tab.payload.view.clone()
        });
        cx.update(|window, app| {
            destination_view.update(app, |document, cx| {
                document.reveal_offset(0, window, cx);
            });
        });

        emit_search_reveal(&workspace, stale_reveal, cx);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.tabs.index_of(&path).is_some());
            let index = workspace.tabs.index_of(&destination).unwrap();
            assert_eq!(workspace.tabs.active_index(), index);
            let tab = workspace.tabs.get(index).unwrap();
            assert!(matches!(
                &tab.identity,
                mt_core::workspace::tabs::TabIdentity::File(saved_path)
                    if saved_path == &destination
            ));
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), recovered_id);
            assert_eq!(active.source_path(), Some(destination.as_path()));
            assert_eq!(active.cursor(app), 0);
        });
    }

    #[gpui_kit::test]
    fn clean_initial_file_accepts_recovery_without_recheckpointing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same-path.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let recovered = "recovered exact 中文 \u{1f680}\n";
        write_recovery_checkpoint(&store, &path, recovered);
        let scans_after_seed = store.retention_scan_count_for_test();
        let store_for_startup = store.clone();

        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, Some(path), move || {
                populated_startup_recovery(store_for_startup)
            });
        cx.run_until_parked();

        let id = workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(document.is_dirty());
            assert_eq!(document.text(app), recovered);
            document.id()
        });
        let restored_at = cx.background_executor.now();
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.recovery_flow.startup_recovery_pending);
            let state = workspace.recovery_flow.recovery_schedules.get(&id).unwrap();
            assert_eq!(
                state.schedule.next_deadline(),
                Some(restored_at + Duration::from_secs(10))
            );
            assert!(state.in_flight.is_none());
            assert!(workspace.recovery_flow._recovery_timer.is_some());
        });

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(
            store.retention_scan_count_for_test(),
            scans_after_seed,
            "a restored checkpoint is already durable and must not be rewritten"
        );

        let latest = "edited after recovery 中文 \u{1f680}\n";
        replace_document(&workspace, 0, latest, cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(store.recover().unwrap().records[0].record.text, latest);
    }

    #[gpui_kit::test]
    fn restored_dirty_checkpoint_refreshes_at_ten_seconds_from_its_durable_baseline(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored-refresh.md");
        fs::write(&path, "disk\n").unwrap();
        let loaded = mt_core::document::io::load(&path).unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let recovered_text = "restored durable 中文 \u{1f680}\n";
        write_recovery_checkpoint(&store, &path, recovered_text);
        let scans_after_seed = store.retention_scan_count_for_test();
        let baseline_checkpointed_at = UNIX_EPOCH + Duration::from_secs(1);
        let scan = RecoveryScan {
            records: vec![RecoveredRecord {
                record: RecoveryRecord {
                    key: RecoveryKey::for_path(&path),
                    text: recovered_text.into(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    checkpointed_at: baseline_checkpointed_at,
                    revision: None,
                },
                source_conflicted: false,
            }],
            issues: Vec::new(),
        };
        let (workspace, cx) = open_test_workspace_with_recovery_store(cx, None, store.clone());
        let restored_at = cx.background_executor.now();

        let restored = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_recovery_for_test(workspace, scan, window, cx)
            })
        });
        assert_eq!(restored, (1, 0));
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
                .recovery_flow
                .recovery_schedules
                .get(&document.id())
                .unwrap();
            assert_eq!(
                state.schedule.next_deadline(),
                Some(restored_at + Duration::from_secs(10))
            );
            assert!(workspace.recovery_flow._recovery_timer.is_some());
        });

        cx.background_executor.advance_clock(Duration::from_secs(9));
        cx.run_until_parked();
        assert_eq!(store.retention_scan_count_for_test(), scans_after_seed);
        assert_eq!(
            store.recover().unwrap().records[0].record.text,
            recovered_text
        );

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(store.retention_scan_count_for_test(), scans_after_seed + 1);
        let refreshed = store.recover().unwrap();
        assert_eq!(refreshed.records[0].record.text, recovered_text);
        assert!(refreshed.records[0].record.checkpointed_at > baseline_checkpointed_at);
    }

    #[gpui_kit::test]
    fn changed_initial_source_restores_recovery_as_conflicted(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changed-source.md");
        fs::write(&path, "original disk\n").unwrap();
        let loaded = mt_core::document::io::load(&path).unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let recovered = "recovered before interruption 中文 \u{1f680}\n";
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key: RecoveryKey::for_path(&path),
                    text: recovered.to_string(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    revision: None,
                },
                &HashSet::new(),
            )
            .unwrap();
        fs::write(&path, "external disk rewrite\n").unwrap();
        let store_for_startup = store.clone();

        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, Some(path.clone()), move || {
                populated_startup_recovery(store_for_startup)
            });
        cx.run_until_parked();

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), recovered);
        });

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();
        assert_eq!(fs::read_to_string(path).unwrap(), "external disk rewrite\n");
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert_eq!(document.text(app), recovered);
        });
    }

    #[gpui_kit::test]
    fn file_opened_during_startup_accepts_matching_recovery(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opened-during-startup.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let recovered = "recovered after manual open\n";
        write_recovery_checkpoint(&store, &path, recovered);
        let startup = populated_startup_recovery(store);
        let (workspace, cx) = open_test_workspace(cx, path);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, HashMap::new(), window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(document.is_dirty());
            assert_eq!(document.text(app), recovered);
        });
    }

    #[gpui_kit::test]
    fn watcher_conflict_survives_startup_recovery_application(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watcher-before-recovery.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let recovered = "recovered while watcher pending\n";
        write_recovery_checkpoint(&store, &path, recovered);
        let startup = populated_startup_recovery(store);
        let (workspace, cx) = open_test_workspace(cx, path);
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| document.mark_externally_changed(cx));

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, startup_targets, window, cx);
            });
        });

        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), recovered);
        });
    }

    #[gpui_kit::test]
    fn queued_retirement_filters_a_startup_record_before_restore(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved-before-recovery.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "obsolete recovery\n");
        let startup = populated_startup_recovery(store.clone());
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let startup_targets =
            workspace.read_with(cx, |workspace, app| workspace.startup_recovery_targets(app));
        let key = RecoveryKey::for_path(&path);

        workspace.update(cx, |workspace, cx| {
            workspace.recovery_flow.recovery = None;
            workspace.recovery_flow.startup_recovery_pending = true;
            workspace.invalidate_recovery(&key, None, cx);
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .contains_key(&key)
            );
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.restore_startup_recovery(startup, startup_targets, window, cx);
            });
        });
        cx.run_until_parked();

        assert!(store.recover().unwrap().records.is_empty());
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(
                workspace.document_at(0).unwrap().read(app).text(app),
                "disk\n"
            );
            assert!(
                workspace
                    .recovery_flow
                    .pending_recovery_retirements
                    .is_empty()
            );
        });
    }

    #[gpui_kit::test]
    fn startup_recovery_counts_each_scan_issue_once(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let scan = RecoveryScan {
            records: Vec::new(),
            issues: vec![RecoveryIssue::Malformed {
                path: dir.path().join("malformed.mtrecovery"),
            }],
        };
        let (workspace, cx) = open_test_workspace_with(cx, None);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                // `startup_recovery` aggregates scan issues with maintenance
                // issues before this layer restores the records.
                restore_startup_recovery_for_test(workspace, scan, 1, None, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Restored 0 recovery checkpoint(s); skipped 1 unavailable or invalid record(s)."
                )
            );
        });
    }

    #[gpui_kit::test]
    fn malformed_startup_recovery_is_reported_without_blocking_editing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("editing-remains-available.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let scan = RecoveryScan {
            records: Vec::new(),
            issues: vec![RecoveryIssue::Malformed {
                path: dir.path().join("malformed.mtrecovery"),
            }],
        };

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_startup_recovery_for_test(workspace, scan, 1, None, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Restored 0 recovery checkpoint(s); skipped 1 unavailable or invalid record(s)."
                )
            );
        });
        replace_document(&workspace, 0, "still editable 中文 \u{1f680}\n", cx);
        assert_eq!(
            document_text(&workspace, 0, cx),
            "still editable 中文 \u{1f680}\n"
        );
    }

    #[test]
    fn startup_recovery_status_keeps_single_signal_messages_and_combines_mixed_outcomes() {
        assert_eq!(startup_recovery_status(0, 0, None), None);
        assert_eq!(
            startup_recovery_status(0, 0, Some("recovery decryption failed")).as_deref(),
            Some("recovery decryption failed. Editing remains available.")
        );
        assert_eq!(
            startup_recovery_status(1, 0, None).as_deref(),
            Some("Restored 1 recovery checkpoint(s); skipped 0 unavailable or invalid record(s).")
        );
        assert_eq!(
            startup_recovery_status(0, 2, None).as_deref(),
            Some("Restored 0 recovery checkpoint(s); skipped 2 unavailable or invalid record(s).")
        );
        assert_eq!(
            startup_recovery_status(0, 1, Some("recovery decryption failed")).as_deref(),
            Some(
                "recovery decryption failed. Editing remains available. Restored 0 recovery checkpoint(s); skipped 1 unavailable or invalid record(s)."
            )
        );
    }

    #[gpui_kit::test]
    fn startup_recovery_reports_error_beside_restored_or_skipped_summary(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup-mixed.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let scan = RecoveryScan {
            records: Vec::new(),
            issues: vec![RecoveryIssue::Malformed {
                path: dir.path().join("malformed.mtrecovery"),
            }],
        };

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_startup_recovery_for_test(
                    workspace,
                    scan,
                    1,
                    Some("recovery decryption failed".into()),
                    window,
                    cx,
                );
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "recovery decryption failed. Editing remains available. Restored 0 recovery checkpoint(s); skipped 1 unavailable or invalid record(s)."
                )
            );
        });
        replace_document(
            &workspace,
            0,
            "still editable after mixed startup 中文 \u{1f680}\n",
            cx,
        );
        assert_eq!(
            document_text(&workspace, 0, cx),
            "still editable after mixed startup 中文 \u{1f680}\n"
        );
    }
    #[test]
    fn revision_answer_binding_invalidates_same_revision_checkpoint() {
        let first_binding = RevisionRequestBinding::new([1; 32], 7, 3, [2; 32], [3; 32], [4; 32]);
        let second_binding = RevisionRequestBinding::new([1; 32], 7, 3, [2; 32], [3; 32], [5; 32]);
        let first = RecoveryContentIdentity {
            revision: 7,
            revision_binding: Some(first_binding),
        };
        let second = RecoveryContentIdentity {
            revision: 7,
            revision_binding: Some(second_binding),
        };
        let written = CheckpointBatchOutcome::Written;

        assert_ne!(first, second);
        assert!(current_checkpoint_write_completed_for_identity(
            true, first, first, &written
        ));
        assert!(!current_checkpoint_write_completed_for_identity(
            true, second, first, &written
        ));
    }

    #[test]
    fn revision_answer_retirement_requires_accepted_representation() {
        let accepted = [(ChangeId(0), true), (ChangeId(1), true)];
        let partial = [(ChangeId(0), true), (ChangeId(1), false)];
        let represented = RevisionQuestionCoverageStatus::Represented {
            change_ids: vec![ChangeId(0), ChangeId(1)],
        };
        let omitted = RevisionQuestionCoverageStatus::IntentionallyOmitted {
            reason: "deliberately absent".to_owned(),
        };

        assert!(revision_answer_is_incorporated(
            &RevisionAnswer::unanswered(),
            None,
            &partial
        ));
        assert!(revision_answer_is_incorporated(
            &RevisionAnswer::intentionally_unspecified(),
            Some(&omitted),
            &partial
        ));
        assert!(revision_answer_is_incorporated(
            &RevisionAnswer::answered("keep this"),
            Some(&represented),
            &accepted
        ));
        assert!(!revision_answer_is_incorporated(
            &RevisionAnswer::answered("keep this"),
            Some(&represented),
            &partial
        ));
        assert!(!revision_answer_is_incorporated(
            &RevisionAnswer::answered("keep this"),
            Some(&omitted),
            &accepted
        ));
    }

    #[test]
    fn revision_apply_identity_blocks_stale_save_after_decision_changes() {
        let reject_all = [(ChangeId(0), false)];
        let accept_all = [(ChangeId(0), true)];

        assert!(revision_apply_identity_matches(
            Some("old"),
            Some(&reject_all),
            "old",
            &reject_all,
        ));
        assert!(!revision_apply_identity_matches(
            Some("old"),
            Some(&reject_all),
            "new",
            &accept_all,
        ));
        assert!(revision_apply_identity_matches(
            Some("new"),
            Some(&accept_all),
            "new",
            &accept_all,
        ));
        assert!(!revision_apply_identity_matches(
            Some("new"),
            Some(&accept_all),
            "old",
            &reject_all,
        ));
    }

    #[gpui::test]
    fn revision_recovered_answers_can_open_review_export_and_discard(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovered-answer-actions.md");
        fs::write(&path, "# Recovery\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let key = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).recovery_key()
        });
        let recovery = RevisionRecovery::new(
            RevisionRequestBinding::new([1; 32], 7, 3, [2; 32], [3; 32], [4; 32]),
            RevisionAnswers::for_recovery(vec![RevisionAnswer::answered("recovered answer text")])
                .unwrap(),
        )
        .with_source_dirty(false);

        workspace.update(cx, |workspace, cx| {
            let document_id = workspace.document_at(0).unwrap().read(cx).id();
            workspace.review_flow.store_recovered_revision_record(
                document_id,
                key.clone(),
                recovery.clone(),
            );
            workspace.open_review_panel(ReviewTarget::Document, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.review_flow.review_panel_is_open());
            assert!(workspace.review_flow.has_recovered_revision_record(&key));
        });

        workspace.update(cx, |workspace, cx| {
            workspace.copy_recovered_revision_answers(cx);
        });
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("recovered answers must be exported as text");
        assert!(copied.contains("recovered answer text"));
        let copied_json: serde_json::Value =
            serde_json::from_str(&copied).expect("recovered answers export must be JSON");
        assert_eq!(
            copied_json["source_revision"],
            serde_json::json!(recovery.binding().source_revision())
        );
        assert_eq!(
            copied_json["source_generation"],
            serde_json::json!(recovery.binding().source_generation())
        );
        assert_eq!(
            copied_json["source_sha256"],
            serde_json::json!(hex_bytes(recovery.binding().source_sha256()))
        );
        assert_eq!(
            copied_json["artifact_lens_sha256"],
            serde_json::json!(hex_bytes(recovery.binding().artifact_lens_digest()))
        );
        assert_eq!(
            copied_json["review_context_sha256"],
            serde_json::json!(hex_bytes(recovery.binding().review_context_digest()))
        );
        assert_eq!(
            copied_json["answers_sha256"],
            serde_json::json!(hex_bytes(recovery.binding().answers_digest()))
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.review_flow.has_recovered_revision_record(&key));
        });

        workspace.update(cx, |workspace, cx| {
            let document_id = workspace.document_at(0).unwrap().read(cx).id();
            workspace.review_flow.store_recovered_revision_record(
                document_id,
                key.clone(),
                recovery,
            );
            workspace.discard_revision_answers(cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.review_flow.has_recovered_revision_record(&key));
        });
    }

    #[gpui_kit::test]
    fn recovered_sibling_survives_reopened_file_checkpoint_save_and_discard(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reopened-recovery.md");
        fs::write(&path, "ordinary disk text\n").unwrap();
        let recovered_text = "independent recovered text\n";
        let recovered_key = RecoveryKey::for_path(&path);
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, recovered_text);
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (recovered_id, _) = restore_open_file_checkpoint(&workspace, &path, &store, cx);
        workspace.update(cx, |workspace, cx| {
            let ordinary = workspace.tabs.index_of(&path).unwrap();
            workspace.close_tab_unchecked(ordinary, cx);
        });

        for (decision, text) in [
            ("Save", "saved live text\n"),
            ("Discard", "discarded live text\n"),
        ] {
            cx.update(|window, app| {
                workspace.update(app, |workspace, cx| {
                    assert!(workspace.open_file(path.clone(), window, cx));
                });
            });
            let ordinary = workspace.read_with(cx, |workspace, _| workspace.tabs.active_index());
            replace_document(&workspace, ordinary, text, cx);
            let checkpoint = workspace.read_with(cx, |workspace, app| {
                workspace
                    .active_document()
                    .unwrap()
                    .read(app)
                    .recovery_checkpoint(app)
            });
            store
                .checkpoint(
                    &checkpoint,
                    &HashSet::from([recovered_key.clone(), checkpoint.key.clone()]),
                )
                .unwrap();

            let scan = store.recover().unwrap();
            let retained = scan
                .records
                .iter()
                .find(|record| record.record.key == recovered_key)
                .unwrap();
            assert_eq!(retained.record.text, recovered_text);
            assert_ne!(checkpoint.key, recovered_key);
            assert!(scan.records.iter().any(|record| {
                record.record.key == checkpoint.key && record.record.text == text
            }));

            cx.simulate_keystrokes("ctrl-w");
            cx.simulate_prompt_answer(decision);
            cx.run_until_parked();
            let remaining = store.recover().unwrap();
            assert_eq!(remaining.records.len(), 1);
            assert_eq!(remaining.records[0].record.key, recovered_key);
            assert_eq!(remaining.records[0].record.text, recovered_text);
            workspace.read_with(cx, |workspace, app| {
                let recovered = workspace.document_by_id(recovered_id, app).unwrap();
                assert_eq!(recovered.read(app).text(app), recovered_text);
            });
        }
    }

    #[gpui::test]
    fn recovered_answers_stay_with_the_recovered_tab_after_reopening_file(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovered-alias.md");
        let disk_text = "ordinary disk text\n";
        let recovered_text = "older recovered text\n";
        fs::write(&path, disk_text).unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let loaded = mt_core::document::io::load(&path).unwrap();
        let recovered_key = RecoveryKey::for_path(&path);
        let recovered_answer = "Keep the recovered intent.";
        let revision_recovery = RevisionRecovery::new(
            RevisionRequestBinding::new([1; 32], 7, 3, [2; 32], [3; 32], [4; 32]),
            RevisionAnswers::for_recovery(vec![RevisionAnswer::answered(recovered_answer)])
                .unwrap(),
        )
        .with_source_dirty(true);
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key: recovered_key.clone(),
                    text: recovered_text.to_owned(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    revision: Some(revision_recovery),
                },
                &HashSet::new(),
            )
            .unwrap();

        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let original_file = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        original_file.update(cx, |document, _| document.rotate_recovery_key());
        let scan = store.recover().unwrap();
        let recovered_document_id = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let result = restore_recovery_for_test(workspace, scan, window, cx);
                assert_eq!(result, (1, 0));
                let recovered = workspace
                    .tabs
                    .iter()
                    .find(|tab| {
                        matches!(
                            &tab.identity,
                            mt_core::workspace::tabs::TabIdentity::Recovered(key)
                                if key == &recovered_key
                        )
                    })
                    .expect("the old checkpoint must have a recovered-only tab");
                recovered.payload.view.read(cx).id()
            })
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let live_index = workspace
                .tabs
                .index_of(&path)
                .expect("the original file tab remains open during restore");
            workspace.close_tab_unchecked(live_index, cx);
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_file(path.clone(), window, cx));
            });
        });
        cx.run_until_parked();

        let (ordinary_document_id, ordinary_key) = workspace.read_with(cx, |workspace, app| {
            let ordinary = workspace.active_document().unwrap().read(app);
            assert_eq!(ordinary.source_path(), Some(path.as_path()));
            (ordinary.id(), ordinary.recovery_key())
        });
        assert_ne!(ordinary_document_id, recovered_document_id);
        assert_ne!(ordinary_key, recovered_key);

        let recovered_answers_offered = cx.update(|window, app| {
            use gpui_kit::test::TestWindowExt as _;

            workspace.update(app, |workspace, cx| {
                workspace.open_review_panel(ReviewTarget::Document, cx);
            });
            window.render_frame(app);
            let offered = window.try_find("revision-recovered-answers").is_some();
            if offered {
                window.click("revision-discard-recovered-answers", app);
            }
            offered
        });
        if !recovered_answers_offered {
            // Exercise the same handler even when the correctly-bound ordinary
            // tab has no recovered-answer button to click.
            workspace.update(cx, |workspace, cx| {
                workspace.discard_recovered_revision_answers(cx);
            });
        }
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            let recovered = workspace
                .document_by_id(recovered_document_id, app)
                .expect("discarding from the ordinary tab must leave the recovered tab open");
            assert_eq!(recovered.read(app).text(app), recovered_text);
            let answers = workspace
                .review_flow
                .recovered_revision_record_for_document(recovered_document_id, &recovered_key)
                .expect("the recovered tab must keep its answer binding");
            assert_eq!(
                answers.1.answers().as_slice(),
                &[RevisionAnswer::answered(recovered_answer)]
            );
            assert!(
                workspace
                    .review_flow
                    .recovered_revision_record_for_document(ordinary_document_id, &ordinary_key)
                    .is_none(),
                "the ordinary tab must not borrow another document's recovered answers"
            );
            assert!(
                workspace
                    .review_flow
                    .recovered_revision_record_for_document(ordinary_document_id, &recovered_key)
                    .is_none(),
                "an explicit recovered key still requires its document's answer binding"
            );
        });
        assert!(
            !recovered_answers_offered,
            "the ordinary file must not show recovered answers from the recovered-only tab"
        );

        let retained = store.recover().unwrap();
        let old_record = retained
            .records
            .iter()
            .find(|record| record.record.key == recovered_key)
            .expect("discard from the ordinary alias must not retire the recovered checkpoint");
        assert_eq!(old_record.record.text, recovered_text);
        assert_eq!(
            old_record
                .record
                .revision
                .as_ref()
                .unwrap()
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered(recovered_answer)]
        );
    }

    #[gpui::test]
    fn revision_copy_rearms_dirty_source_and_clears_revision_envelope(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("copy-dirty-revision.md");
        fs::write(&path, "disk source\n").unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        replace_document(&workspace, 0, "dirty source survives copy\n", cx);
        let key = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).recovery_key()
        });
        let recovery = RevisionRecovery::new(
            RevisionRequestBinding::new([7; 32], 4, 2, [8; 32], [9; 32], [10; 32]),
            RevisionAnswers::for_recovery(vec![RevisionAnswer::answered("keep intent")]).unwrap(),
        )
        .with_source_dirty(true);
        workspace.update(cx, |workspace, cx| {
            let document_id = workspace.document_at(0).unwrap().read(cx).id();
            workspace.review_flow.store_recovered_revision_record(
                document_id,
                key.clone(),
                recovery,
            );
        });

        workspace.update(cx, |workspace, cx| {
            workspace.copy_recovered_revision_answers(cx);
        });
        assert!(workspace.read_with(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_schedules
                .values()
                .any(|state| state.key == key)
        }));

        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(now + Duration::from_secs(1), cx);
        });
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(
            recovered.records[0].record.text,
            "dirty source survives copy\n"
        );
        assert!(recovered.records[0].record.revision.is_none());
    }

    #[gpui::test]
    fn reject_all_apply_then_accept_save_writes_latest_preview(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("revision-save-order.md");
        fs::write(&path, "old\n").unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (document_id, source_snapshot, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                document.async_snapshot(app),
                document.revision(),
            )
        });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Prompt,
            Some(&path),
            "old\n",
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let question = ClarificationQuestion::new(
            "Which text should be retained?",
            ClarificationPriority::Critical,
        );
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: ReviewSections {
                stated_goal: "update the text".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "updated text".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![question.clone()],
        };
        let answer_states = vec![RevisionAnswer::answered("keep")];
        let answers = RevisionAnswers::new(answer_states.clone()).unwrap();
        let recovery = super::build_revision_recovery(&request, &output, &answer_states)
            .expect("the real Review envelope must be recoverable");
        let checkpoint = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let mut checkpoint = document.recovery_checkpoint(app);
            checkpoint.revision = Some(recovery);
            checkpoint
        });
        let old_recovery_key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([old_recovery_key.clone()]))
            .unwrap();
        assert_eq!(store.recover().unwrap().records.len(), 1);
        let question_id = revision_question_id(0, &question);
        let raw_response = serde_json::json!({
            "schema_version": mt_core::review::provider::REVISION_SCHEMA_VERSION,
            "groups": [{
                "rationale": "Apply the requested wording",
                "edits": [{
                    "range": {"start": 0, "end": 3},
                    "expected_source": "old",
                    "replacement": "new"
                }]
            }],
            "question_coverage": [{
                "question_index": 0,
                "question_id": question_id,
                "status": {"kind": "represented", "change_ids": [0]}
            }]
        })
        .to_string();
        let transport = decode_revision_capture(&request, &output, &answers, &raw_response)
            .unwrap()
            .into_transport_result_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace
                    .review_flow
                    .install_revision_context(super::WorkspaceRevisionContext {
                        document_id,
                        source_snapshot: source_snapshot.clone(),
                        request: request.clone(),
                        review_output: output.clone(),
                        skill_package: None,
                        supporting_sources_current: true,
                        applied: None,
                        answers_exported: false,
                        answer_states: vec![RevisionAnswer::answered("keep")],
                        answer_inputs: Vec::new(),
                    });
                workspace
                    .review_flow
                    .replace_revision_result(super::WorkspaceRevisionResult {
                        document_id,
                        source_snapshot,
                        result: transport,
                        decisions: vec![(ChangeId(0), false)],
                        preview: "old\n".to_owned(),
                    });
                assert!(!workspace.apply_revision(window, cx));
            });
        });
        assert_eq!(document_text(&workspace, 0, cx), "old\n");

        workspace.update(cx, |workspace, cx| {
            workspace.set_revision_decision(ChangeId(0), true, cx);
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_revision(window, cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(document_text(&workspace, 0, cx), "old\n");
        let pre_apply_recovered = store.recover().unwrap();
        assert!(
            pre_apply_recovered
                .records
                .iter()
                .any(|record| record.record.key == old_recovery_key)
        );
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(!document.is_dirty());
            assert!(
                workspace
                    .review_flow
                    .revision_context()
                    .is_some_and(|context| context.applied.is_none())
            );
            assert!(workspace.review_flow.has_revision_result());
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.apply_revision(window, cx));
            });
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.document_at(0).unwrap().read(app).is_dirty());
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_revision(window, cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(document_text(&workspace, 0, cx), "new\n");
        let recovered = store.recover().unwrap();
        assert!(
            recovered
                .records
                .iter()
                .all(|record| record.record.key != old_recovery_key)
        );
        assert!(recovered.records.is_empty());
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(path.as_path()));
            assert!(!workspace.review_flow.has_revision_context());
            assert!(!workspace.review_flow.has_revision_result());
            assert!(!document.is_dirty());
        });
    }

    #[gpui::test]
    fn reject_all_apply_then_accept_save_as_writes_latest_preview(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("revision-save-as-order.md");
        let destination = directory.path().join("revision-save-as-target.md");
        fs::write(&path, "old\n").unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (document_id, source_snapshot, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                document.async_snapshot(app),
                document.revision(),
            )
        });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Prompt,
            Some(&path),
            "old\n",
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let question = ClarificationQuestion::new(
            "Which text should be retained?",
            ClarificationPriority::Critical,
        );
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: ReviewSections {
                stated_goal: "update the text".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "updated text".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![question.clone()],
        };
        let answer_states = vec![RevisionAnswer::answered("keep")];
        let answers = RevisionAnswers::new(answer_states.clone()).unwrap();
        let recovery = super::build_revision_recovery(&request, &output, &answer_states)
            .expect("the real Review envelope must be recoverable");
        let checkpoint = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let mut checkpoint = document.recovery_checkpoint(app);
            checkpoint.revision = Some(recovery);
            checkpoint
        });
        let old_recovery_key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([old_recovery_key.clone()]))
            .unwrap();
        assert_eq!(store.recover().unwrap().records.len(), 1);
        let question_id = revision_question_id(0, &question);
        let raw_response = serde_json::json!({
            "schema_version": mt_core::review::provider::REVISION_SCHEMA_VERSION,
            "groups": [{
                "rationale": "Apply the requested wording",
                "edits": [{
                    "range": {"start": 0, "end": 3},
                    "expected_source": "old",
                    "replacement": "new"
                }]
            }],
            "question_coverage": [{
                "question_index": 0,
                "question_id": question_id,
                "status": {"kind": "represented", "change_ids": [0]}
            }]
        })
        .to_string();
        let transport = decode_revision_capture(&request, &output, &answers, &raw_response)
            .unwrap()
            .into_transport_result_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace
                    .review_flow
                    .install_revision_context(super::WorkspaceRevisionContext {
                        document_id,
                        source_snapshot: source_snapshot.clone(),
                        request: request.clone(),
                        review_output: output.clone(),
                        skill_package: None,
                        supporting_sources_current: true,
                        applied: None,
                        answers_exported: false,
                        answer_states: vec![RevisionAnswer::answered("keep")],
                        answer_inputs: Vec::new(),
                    });
                workspace
                    .review_flow
                    .replace_revision_result(super::WorkspaceRevisionResult {
                        document_id,
                        source_snapshot,
                        result: transport,
                        decisions: vec![(ChangeId(0), false)],
                        preview: "old\n".to_owned(),
                    });
                assert!(!workspace.apply_revision(window, cx));
            });
        });
        assert_eq!(document_text(&workspace, 0, cx), "old\n");

        workspace.update(cx, |workspace, cx| {
            workspace.set_revision_decision(ChangeId(0), true, cx);
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert!(!destination.exists());
        assert_eq!(document_text(&workspace, 0, cx), "old\n");
        let pre_apply_recovered = store.recover().unwrap();
        assert!(
            pre_apply_recovered
                .records
                .iter()
                .any(|record| record.record.key == old_recovery_key)
        );
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(path.as_path()));
            assert!(!document.is_dirty());
            assert!(
                workspace
                    .review_flow
                    .revision_context()
                    .is_some_and(|context| context.applied.is_none())
            );
            assert!(workspace.review_flow.has_revision_result());
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.apply_revision(window, cx));
            });
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.document_at(0).unwrap().read(app).is_dirty());
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, directory.path());
            Some(destination.clone())
        });
        cx.run_until_parked();

        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(fs::read(&destination).unwrap(), b"new\n");
        let recovered = store.recover().unwrap();
        assert!(
            recovered
                .records
                .iter()
                .all(|record| record.record.key != old_recovery_key)
        );
        assert!(recovered.records.is_empty());
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), Some(destination.as_path()));
            assert_eq!(document.text(app), "new\n");
            assert!(!workspace.review_flow.has_revision_context());
            assert!(!workspace.review_flow.has_revision_result());
            assert!(!document.is_dirty());
        });
    }

    fn prepare_reject_all_revision_for_save_as(
        workspace: &Entity<Workspace>,
        path: Option<&Path>,
        store: &RecoveryStore,
        cx: &mut VisualTestContext,
    ) -> (super::DocumentId, RecoveryKey) {
        let (document_id, source_snapshot, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                document.async_snapshot(app),
                document.revision(),
            )
        });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Prompt,
            path,
            "old\n",
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let question = ClarificationQuestion::new(
            "Which text should be retained?",
            ClarificationPriority::Critical,
        );
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: ReviewSections {
                stated_goal: "update the text".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "updated text".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![question.clone()],
        };
        let answer_states = vec![RevisionAnswer::answered("keep")];
        let answers = RevisionAnswers::new(answer_states.clone()).unwrap();
        let recovery = super::build_revision_recovery(&request, &output, &answer_states)
            .expect("the Review envelope must preserve the authored answer");
        let checkpoint = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let mut checkpoint = document.recovery_checkpoint(app);
            checkpoint.revision = Some(recovery);
            checkpoint
        });
        let recovery_key = checkpoint.key.clone();
        store
            .checkpoint(&checkpoint, &HashSet::from([recovery_key.clone()]))
            .unwrap();
        let question_id = revision_question_id(0, &question);
        let raw_response = serde_json::json!({
            "schema_version": mt_core::review::provider::REVISION_SCHEMA_VERSION,
            "groups": [{
                "rationale": "Apply the requested wording",
                "edits": [{
                    "range": {"start": 0, "end": 3},
                    "expected_source": "old",
                    "replacement": "new"
                }]
            }],
            "question_coverage": [{
                "question_index": 0,
                "question_id": question_id,
                "status": {"kind": "represented", "change_ids": [0]}
            }]
        })
        .to_string();
        let transport = decode_revision_capture(&request, &output, &answers, &raw_response)
            .unwrap()
            .into_transport_result_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace
                    .review_flow
                    .install_revision_context(super::WorkspaceRevisionContext {
                        document_id,
                        source_snapshot: source_snapshot.clone(),
                        request: request.clone(),
                        review_output: output.clone(),
                        skill_package: None,
                        supporting_sources_current: true,
                        applied: None,
                        answers_exported: false,
                        answer_states: answer_states.clone(),
                        answer_inputs: Vec::new(),
                    });
                workspace
                    .review_flow
                    .replace_revision_result(super::WorkspaceRevisionResult {
                        document_id,
                        source_snapshot,
                        result: transport,
                        decisions: vec![(ChangeId(0), false)],
                        preview: "old\n".to_owned(),
                    });
                assert!(!workspace.apply_revision(window, cx));
            });
        });
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), "old\n");
            assert_eq!(document.is_dirty(), path.is_none());
            assert!(workspace.review_flow.revision_is_applied(document_id));
        });
        (document_id, recovery_key)
    }

    fn add_secondary_memory_tab(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> mt_core::document::lifecycle::DocumentId {
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.new_memory("other tab\n".to_owned(), window, cx);
                workspace
                    .active_document()
                    .expect("the secondary memory tab becomes active")
                    .read(cx)
                    .id()
            })
        })
    }

    #[gpui::test]
    fn revision_save_as_survives_other_tab_edits_but_not_its_own(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("reviewed-a.md");
        let approved_copy = directory.path().join("approved-a.md");
        fs::write(&source, "old\n").unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(source.clone()), store.clone());
        let (document_id, _) =
            prepare_reject_all_revision_for_save_as(&workspace, Some(&source), &store, cx);
        let other_document_id = add_secondary_memory_tab(&workspace, cx);
        assert_ne!(document_id, other_document_id);

        replace_document(&workspace, 1, "other tab edit\n", cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace.review_flow.revision_is_applied(document_id),
                "another tab's edit must not revoke A's approval"
            );
        });
        workspace.update(cx, |workspace, cx| {
            workspace.focus_path(&source, cx);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, directory.path());
            Some(approved_copy.clone())
        });
        cx.run_until_parked();

        assert_eq!(
            fs::read(&approved_copy).unwrap(),
            b"old\n",
            "editing another tab must not revoke A's approved Save As"
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.review_flow.revision_is_applied(document_id));
        });

        replace_document(&workspace, 0, "A's own edit\n", cx);
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), "A's own edit\n");
            assert!(document.is_dirty());
            assert!(!workspace.review_flow.revision_is_applied(document_id));
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.pending_save_as.is_none());
        });
    }

    #[gpui::test]
    fn revision_save_as_rejects_clean_reload_during_picker_and_replace(cx: &mut TestAppContext) {
        let picker_directory = tempfile::tempdir().unwrap();
        let picker_source = picker_directory.path().join("picker-source.md");
        let picker_destination = picker_directory.path().join("picker-output.md");
        fs::write(&picker_source, "old\n").unwrap();
        let picker_store = RecoveryStore::new_at(
            picker_directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (picker_workspace, cx) = open_test_workspace_with_recovery_store(
            cx,
            Some(picker_source.clone()),
            picker_store.clone(),
        );
        let (picker_document_id, picker_recovery_key) = prepare_reject_all_revision_for_save_as(
            &picker_workspace,
            Some(&picker_source),
            &picker_store,
            cx,
        );
        let document = picker_workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let source_stamp_before_reload =
            document.read_with(cx, |document, _| document.source_stamp());
        cx.update(|window, app| {
            picker_workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        fs::write(&picker_source, "old\n").unwrap();
        cx.update(|window, app| {
            document.update(app, |document, cx| document.reload(window, cx));
        });
        let source_stamp_after_reload =
            document.read_with(cx, |document, _| document.source_stamp());
        assert_ne!(
            source_stamp_before_reload, source_stamp_after_reload,
            "even a same-text clean reload advances the document source stamp"
        );
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, picker_directory.path());
            Some(picker_destination.clone())
        });
        cx.run_until_parked();

        assert!(!picker_destination.exists());
        assert_eq!(fs::read(&picker_source).unwrap(), b"old\n");
        picker_workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.id(), picker_document_id);
            assert_eq!(document.source_path(), Some(picker_source.as_path()));
            assert_eq!(document.text(app), "old\n");
            assert!(!document.is_dirty());
            assert!(workspace.pending_save_as.is_none());
            assert!(
                workspace
                    .review_flow
                    .revision_context()
                    .is_some_and(|context| context.applied.is_none())
            );
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![RevisionAnswer::answered("keep")]
            );
            assert!(workspace.review_flow.has_revision_result());
            assert!(workspace.review_flow.revision_diagnostic().is_some());
        });
        let picker_recovered = picker_store.recover().unwrap();
        let picker_record = picker_recovered
            .records
            .iter()
            .find(|record| record.record.key == picker_recovery_key)
            .expect("a stale Save As must preserve answer recovery");
        assert_eq!(
            picker_record
                .record
                .revision
                .as_ref()
                .unwrap()
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered("keep")]
        );

        let replace_directory = tempfile::tempdir().unwrap();
        let replace_source = replace_directory.path().join("replace-source.md");
        let replace_destination = replace_directory.path().join("existing-output.md");
        let original_destination = b"keep existing destination\n";
        let externally_reloaded = "new unapproved source text\n";
        fs::write(&replace_source, "old\n").unwrap();
        fs::write(&replace_destination, original_destination).unwrap();
        let replace_store = RecoveryStore::new_at(
            replace_directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (replace_workspace, cx) = open_test_workspace_with_recovery_store(
            cx,
            Some(replace_source.clone()),
            replace_store.clone(),
        );
        let (replace_document_id, replace_recovery_key) = prepare_reject_all_revision_for_save_as(
            &replace_workspace,
            Some(&replace_source),
            &replace_store,
            cx,
        );
        let document = replace_workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let source_stamp_before_reload =
            document.read_with(cx, |document, _| document.source_stamp());
        cx.update(|window, app| {
            replace_workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, replace_directory.path());
            Some(replace_destination.clone())
        });
        cx.run_until_parked();
        assert!(
            cx.has_pending_prompt(),
            "the existing target requires Replace"
        );

        fs::write(&replace_source, externally_reloaded).unwrap();
        cx.update(|window, app| {
            document.update(app, |document, cx| document.reload(window, cx));
        });
        let source_stamp_after_reload =
            document.read_with(cx, |document, _| document.source_stamp());
        assert_ne!(source_stamp_before_reload, source_stamp_after_reload);
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();

        assert_eq!(
            fs::read(&replace_destination).unwrap(),
            original_destination
        );
        assert_eq!(
            fs::read(&replace_source).unwrap(),
            externally_reloaded.as_bytes()
        );
        replace_workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.id(), replace_document_id);
            assert_eq!(document.source_path(), Some(replace_source.as_path()));
            assert_eq!(document.text(app), externally_reloaded);
            assert!(!document.is_dirty());
            assert!(workspace.pending_save_as.is_none());
            assert!(
                workspace
                    .review_flow
                    .revision_context()
                    .is_some_and(|context| context.applied.is_none())
            );
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![RevisionAnswer::answered("keep")]
            );
            assert!(workspace.review_flow.has_revision_result());
            assert!(workspace.review_flow.revision_diagnostic().is_some());
        });
        let replace_recovered = replace_store.recover().unwrap();
        let replace_record = replace_recovered
            .records
            .iter()
            .find(|record| record.record.key == replace_recovery_key)
            .expect("Replace after a stale approval must preserve answer recovery");
        assert_eq!(
            replace_record
                .record
                .revision
                .as_ref()
                .unwrap()
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered("keep")]
        );

        let pathless_directory = tempfile::tempdir().unwrap();
        let pathless_destination = pathless_directory.path().join("pathless-output.md");
        let pathless_store = RecoveryStore::new_at(
            pathless_directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (pathless_workspace, cx) =
            open_test_workspace_with_recovery_store(cx, None, pathless_store.clone());
        let created_pathless_document_id = cx.update(|window, app| {
            pathless_workspace.update(app, |workspace, cx| {
                workspace.new_memory("old\n".to_owned(), window, cx);
                workspace.active_document().unwrap().read(cx).id()
            })
        });
        let (pathless_document_id, pathless_recovery_key) =
            prepare_reject_all_revision_for_save_as(&pathless_workspace, None, &pathless_store, cx);
        assert_eq!(pathless_document_id, created_pathless_document_id);
        cx.update(|window, app| {
            pathless_workspace.update(app, |workspace, cx| {
                workspace.save_revision(window, cx);
            });
        });
        replace_document(&pathless_workspace, 0, "new unapproved memory text\n", cx);
        cx.simulate_new_path_selection(|_| Some(pathless_destination.clone()));
        cx.run_until_parked();

        assert!(
            !pathless_destination.exists(),
            "Revision Save on a memory document must not downgrade to an ordinary Save As"
        );
        pathless_workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_by_id(pathless_document_id, app).unwrap();
            let document = document.read(app);
            assert_eq!(document.text(app), "new unapproved memory text\n");
            assert_eq!(document.source_path(), None);
            assert!(document.is_dirty());
            assert!(workspace.pending_save_as.is_none());
            assert!(
                workspace
                    .review_flow
                    .revision_context()
                    .is_some_and(|context| context.applied.is_none())
            );
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![RevisionAnswer::answered("keep")]
            );
            assert!(workspace.review_flow.has_revision_result());
        });
        let pathless_recovered = pathless_store.recover().unwrap();
        let pathless_record = pathless_recovered
            .records
            .iter()
            .find(|record| record.record.key == pathless_recovery_key)
            .expect("a stale pathless Revision Save must retain answer recovery");
        assert_eq!(
            pathless_record
                .record
                .revision
                .as_ref()
                .unwrap()
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered("keep")]
        );

        let picker_switch_directory = tempfile::tempdir().unwrap();
        let picker_switch_source = picker_switch_directory.path().join("source.md");
        let picker_switch_destination = picker_switch_directory.path().join("picker-output.md");
        fs::write(&picker_switch_source, "old\n").unwrap();
        let picker_switch_store = RecoveryStore::new_at(
            picker_switch_directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (picker_switch_workspace, cx) = open_test_workspace_with_recovery_store(
            cx,
            Some(picker_switch_source.clone()),
            picker_switch_store.clone(),
        );
        let (picker_switch_document_id, _) = prepare_reject_all_revision_for_save_as(
            &picker_switch_workspace,
            Some(&picker_switch_source),
            &picker_switch_store,
            cx,
        );
        cx.update(|window, app| {
            picker_switch_workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        let picker_switch_active_id = add_secondary_memory_tab(&picker_switch_workspace, cx);
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, picker_switch_directory.path());
            Some(picker_switch_destination.clone())
        });
        cx.run_until_parked();

        assert_eq!(fs::read(&picker_switch_destination).unwrap(), b"old\n");
        picker_switch_workspace.read_with(cx, |workspace, app| {
            let document = workspace
                .document_by_id(picker_switch_document_id, app)
                .unwrap();
            let document = document.read(app);
            assert_eq!(
                document.source_path(),
                Some(picker_switch_destination.as_path())
            );
            assert_eq!(document.text(app), "old\n");
            assert_eq!(
                workspace.active_document().unwrap().read(app).id(),
                picker_switch_active_id
            );
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![RevisionAnswer::answered("keep")]
            );
            assert!(workspace.pending_save_as.is_none());
        });

        let replace_switch_directory = tempfile::tempdir().unwrap();
        let replace_switch_source = replace_switch_directory.path().join("source.md");
        let replace_switch_destination = replace_switch_directory.path().join("existing.md");
        fs::write(&replace_switch_source, "old\n").unwrap();
        fs::write(&replace_switch_destination, "original destination\n").unwrap();
        let replace_switch_store = RecoveryStore::new_at(
            replace_switch_directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (replace_switch_workspace, cx) = open_test_workspace_with_recovery_store(
            cx,
            Some(replace_switch_source.clone()),
            replace_switch_store.clone(),
        );
        let (replace_switch_document_id, _) = prepare_reject_all_revision_for_save_as(
            &replace_switch_workspace,
            Some(&replace_switch_source),
            &replace_switch_store,
            cx,
        );
        cx.update(|window, app| {
            replace_switch_workspace.update(app, |workspace, cx| {
                workspace.save_as_revision(window, cx);
            });
        });
        cx.simulate_new_path_selection(|parent| {
            assert_eq!(parent, replace_switch_directory.path());
            Some(replace_switch_destination.clone())
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        let replace_switch_active_id = add_secondary_memory_tab(&replace_switch_workspace, cx);
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();

        assert_eq!(fs::read(&replace_switch_destination).unwrap(), b"old\n");
        replace_switch_workspace.read_with(cx, |workspace, app| {
            let document = workspace
                .document_by_id(replace_switch_document_id, app)
                .unwrap();
            let document = document.read(app);
            assert_eq!(
                document.source_path(),
                Some(replace_switch_destination.as_path())
            );
            assert_eq!(document.text(app), "old\n");
            assert_eq!(
                workspace.active_document().unwrap().read(app).id(),
                replace_switch_active_id
            );
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![RevisionAnswer::answered("keep")]
            );
            assert!(workspace.pending_save_as.is_none());
        });
    }

    #[gpui::test]
    fn revision_copy_after_apply_preserves_clipboard_when_source_turns_stale(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("revision-copy-stale.md");
        fs::write(&path, "old\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let (document_id, source_snapshot, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (
                document.id(),
                document.async_snapshot(app),
                document.revision(),
            )
        });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Prompt,
            Some(&path),
            "old\n",
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let question = ClarificationQuestion::new(
            "Which text should be retained?",
            ClarificationPriority::Critical,
        );
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: ReviewSections {
                stated_goal: "update the text".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "updated text".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![question.clone()],
        };
        let answer_states = vec![RevisionAnswer::answered("keep")];
        let answers = RevisionAnswers::new(answer_states.clone()).unwrap();
        let question_id = revision_question_id(0, &question);
        let raw_response = serde_json::json!({
            "schema_version": mt_core::review::provider::REVISION_SCHEMA_VERSION,
            "groups": [{
                "rationale": "Apply the requested wording",
                "edits": [{
                    "range": {"start": 0, "end": 3},
                    "expected_source": "old",
                    "replacement": "new"
                }]
            }],
            "question_coverage": [{
                "question_index": 0,
                "question_id": question_id,
                "status": {"kind": "represented", "change_ids": [0]}
            }]
        })
        .to_string();
        let transport = decode_revision_capture(&request, &output, &answers, &raw_response)
            .unwrap()
            .into_transport_result_for_test();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.review_flow.set_review_panel_open(true);
                workspace.right_panel_open = true;
                workspace.review_flow.install_review_result(
                    super::WorkspaceReviewResult {
                        document_id,
                        source_snapshot: source_snapshot.clone(),
                        target: ReviewTarget::Document,
                        selection: None,
                        lens: ArtifactLens::Prompt,
                        partial: false,
                        skill_package: None,
                        supporting_sources_current: true,
                        result: mt_core::review::provider::ReviewTransportResult {
                            result: mt_core::review::ReviewResult::ready(&request, output.clone())
                                .unwrap(),
                            metadata: mt_core::review::provider::ReviewMetadata::from_response(
                                mt_core::model::Provider::OpenAiResponses,
                                "test-model",
                                "test-model",
                            )
                            .unwrap(),
                        },
                    },
                    false,
                );
                workspace
                    .review_flow
                    .install_revision_context(super::WorkspaceRevisionContext {
                        document_id,
                        source_snapshot: source_snapshot.clone(),
                        request,
                        review_output: output,
                        skill_package: None,
                        supporting_sources_current: true,
                        applied: None,
                        answers_exported: false,
                        answer_states,
                        answer_inputs: Vec::new(),
                    });
                workspace
                    .review_flow
                    .replace_revision_result(super::WorkspaceRevisionResult {
                        document_id,
                        source_snapshot,
                        result: transport,
                        decisions: vec![(ChangeId(0), true)],
                        preview: "new\n".to_owned(),
                    });
                assert!(workspace.apply_revision(window, cx));
            });
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            workspace.copy_revision(cx);
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("new\n".to_owned())
        );
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), "new\n");
            assert!(document.is_dirty());
        });
        assert_eq!(fs::read(&path).unwrap(), b"old\n");

        replace_document(&workspace, 0, "manual edit\n", cx);
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel".to_owned()));
        workspace.update(cx, |workspace, cx| {
            workspace.copy_revision(cx);
        });

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("sentinel".to_owned())
        );
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.review_flow.review_panel_is_open());
            assert!(workspace.right_panel_open);
            assert_eq!(
                workspace.review_flow.revision_diagnostic(),
                Some(i18n::t(i18n::Key::RevisionStale, app))
            );
        });
        cx.update(|window, app| {
            use gpui_kit::test::TestWindowExt as _;

            window.render_frame(app);
            let stale = window.find("revision-stale");
            assert!(stale.visible());
            assert_eq!(
                stale.label(),
                Some(i18n::t(i18n::Key::RevisionStaleInspection, app))
            );
            window.click("revision-apply", app);
        });
        cx.run_until_parked();
        assert_eq!(document_text(&workspace, 0, cx), "manual edit\n");
    }

    #[gpui::test]
    fn revision_restored_answers_ignore_runtime_counters_and_mismatches_stay_quarantined(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restored-revision.md");
        let text = "# Restored\n\nKeep this intent.\n";
        let old_recovered_text = "# Older recovered body\n\nKeep this quarantined answer.\n";
        let old_recovered_answer = "Keep the original intent and heading.";
        fs::write(&path, text).unwrap();
        let store = RecoveryStore::new_at(
            directory.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(path.clone()), store.clone());
        let (document, document_id, recovery_key, revision, source_snapshot) =
            workspace.read_with(cx, |workspace, app| {
                let document = workspace.document_at(0).unwrap();
                let document_read = document.read(app);
                (
                    document.clone(),
                    document_read.id(),
                    document_read.recovery_key(),
                    document_read.revision(),
                    document_read.async_snapshot(app),
                )
            });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Plan,
            Some(&path),
            text,
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let output = mt_core::review::ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: mt_core::review::ReviewSections {
                stated_goal: "preserve the document intent".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "a clearer document".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![mt_core::review::ClarificationQuestion::new(
                "What must remain unchanged?",
                mt_core::review::ClarificationPriority::High,
            )],
        };
        let recovery = super::build_revision_recovery(
            &request,
            &output,
            &[RevisionAnswer::answered(old_recovered_answer)],
        )
        .unwrap();
        let binding = recovery.binding();
        let shifted_recovery = RevisionRecovery::new(
            RevisionRequestBinding::new(
                *binding.source_sha256(),
                binding.source_revision().wrapping_add(100),
                binding.source_generation().wrapping_add(100),
                *binding.artifact_lens_digest(),
                *binding.review_context_digest(),
                *binding.answers_digest(),
            ),
            recovery.answers().clone(),
        );
        let mismatched_recovery = RevisionRecovery::new(
            RevisionRequestBinding::new(
                [9; 32],
                binding.source_revision(),
                binding.source_generation(),
                *binding.artifact_lens_digest(),
                *binding.review_context_digest(),
                *binding.answers_digest(),
            ),
            recovery.answers().clone(),
        );
        let mismatched_checkpoint = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let mut checkpoint = document.recovery_checkpoint(app);
            checkpoint.text = old_recovered_text.to_owned();
            checkpoint.revision = Some(mismatched_recovery.clone());
            checkpoint
        });
        store
            .checkpoint(
                &mismatched_checkpoint,
                &HashSet::from([recovery_key.clone()]),
            )
            .unwrap();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.review_flow.store_recovered_revision_record(
                    document_id,
                    recovery_key.clone(),
                    shifted_recovery,
                );
                workspace.install_revision_context(
                    document_id,
                    recovery_key.clone(),
                    source_snapshot.clone(),
                    request.clone(),
                    output.clone(),
                    None,
                    true,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            let context = workspace
                .review_flow
                .revision_context()
                .expect("the revision context must be installed");
            assert_eq!(
                context.answer_states,
                vec![RevisionAnswer::answered(
                    "Keep the original intent and heading."
                )]
            );
            assert_eq!(
                context.answer_inputs[0].read(app).value().as_ref(),
                "Keep the original intent and heading."
            );
            assert!(
                !workspace
                    .review_flow
                    .has_recovered_revision_record(&recovery_key)
            );
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.review_flow.store_recovered_revision_record(
                    document_id,
                    recovery_key.clone(),
                    mismatched_recovery.clone(),
                );
                workspace.install_revision_context(
                    document_id,
                    recovery_key.clone(),
                    source_snapshot,
                    request,
                    output,
                    None,
                    true,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            let context = workspace
                .review_flow
                .revision_context()
                .expect("the mismatched context must remain visible but unanswered");
            assert_eq!(context.answer_states, vec![RevisionAnswer::unanswered()]);
            assert!(context.answer_inputs[0].read(app).value().is_empty());
            assert_eq!(
                workspace
                    .review_flow
                    .recovered_revision_record_for_document(document_id, &recovery_key)
                    .map(|(_, recovery)| *recovery.binding()),
                Some(*mismatched_recovery.binding())
            );
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.set_revision_answer_state(
                    0,
                    RevisionAnswer::answered("Keep the new answer."),
                    window,
                    cx,
                );
            });
        });
        workspace.read_with(cx, |workspace, _| {
            let live = workspace
                .review_flow
                .revision_recovery_for_document(document_id, &recovery_key)
                .expect("the live answer must supersede quarantined recovery for checkpointing");
            assert_eq!(
                live.answers().as_slice(),
                &[RevisionAnswer::answered("Keep the new answer.")]
            );
            assert_ne!(live.binding(), mismatched_recovery.binding());
        });

        let live_text = "# Edited after the new answer\n\nKeep the new source text.\n";
        replace_document(&workspace, 0, live_text, cx);
        let live_key = document.read_with(cx, |document, _| document.recovery_key());
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        let scan = store.recover().unwrap();
        assert_eq!(
            scan.records.len(),
            2,
            "the quarantined record and live checkpoint must remain independently durable"
        );
        {
            let old_record = scan
                .records
                .iter()
                .find(|record| record.record.key == recovery_key)
                .expect("the quarantined record must retain its original key");
            assert_eq!(old_record.record.text, old_recovered_text);
            let old_persisted = old_record
                .record
                .revision
                .as_ref()
                .expect("the quarantined answers must remain inspectable");
            assert_eq!(
                old_persisted.answers().as_slice(),
                &[RevisionAnswer::answered(old_recovered_answer)]
            );
            assert_eq!(old_persisted.binding(), mismatched_recovery.binding());

            assert_ne!(live_key, recovery_key);
            let live_record = scan
                .records
                .iter()
                .find(|record| record.record.key == live_key)
                .expect("the live checkpoint must use its distinct incarnation key");
            assert_eq!(live_record.record.text, live_text);
            let live_persisted = live_record
                .record
                .revision
                .as_ref()
                .expect("the live answer must be durable with its text");
            assert_eq!(
                live_persisted.answers().as_slice(),
                &[RevisionAnswer::answered("Keep the new answer.")]
            );
            assert_ne!(live_persisted.binding(), mismatched_recovery.binding());
        }

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_startup_recovery_for_test(workspace, scan, 0, None, window, cx);
            })
        });
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 3);
            assert_eq!(workspace.tabs.index_of(&path), Some(0));
            let recovered_tabs = workspace
                .tabs
                .iter()
                .filter_map(|tab| match &tab.identity {
                    mt_core::workspace::tabs::TabIdentity::Recovered(key) => {
                        Some((key.clone(), tab.payload.view.clone()))
                    }
                    _ => None,
                })
                .collect::<HashMap<_, _>>();
            assert_eq!(recovered_tabs.len(), 2);

            let old_view = recovered_tabs
                .get(&recovery_key)
                .expect("the old answer checkpoint must have its own recovered tab");
            let old_document = old_view.read(app);
            assert_eq!(old_document.text(app), old_recovered_text);
            let old_answers = workspace
                .review_flow
                .revision_recovery_for_document(old_document.id(), &recovery_key)
                .expect("the old quarantined answers must remain user-accessible");
            assert_eq!(
                old_answers.answers().as_slice(),
                &[RevisionAnswer::answered(old_recovered_answer)]
            );

            let live_view = recovered_tabs
                .get(&live_key)
                .expect("the live answer checkpoint must have its own recovered tab");
            let live_document = live_view.read(app);
            assert_eq!(live_document.text(app), live_text);
            let live_answers = workspace
                .review_flow
                .revision_recovery_for_document(live_document.id(), &live_key)
                .expect("the live answer must remain user-accessible");
            assert_eq!(
                live_answers.answers().as_slice(),
                &[RevisionAnswer::answered("Keep the new answer.")]
            );
        });

        let ordinary_live_text =
            "# Edited ordinary file after recovery\n\nKeep this newer source.\n";
        replace_document(&workspace, 0, ordinary_live_text, cx);
        let ordinary_key = document.read_with(cx, |document, _| document.recovery_key());
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        let updated_scan = store.recover().unwrap();
        assert_eq!(
            updated_scan.records.len(),
            3,
            "editing the ordinary tab must not overwrite either recovered-only checkpoint"
        );
        let old_record = updated_scan
            .records
            .iter()
            .find(|record| record.record.key == recovery_key)
            .expect("the old path-key checkpoint must remain durable");
        assert_eq!(old_record.record.text, old_recovered_text);
        let old_persisted = old_record
            .record
            .revision
            .as_ref()
            .expect("the old quarantined answer must remain inspectable");
        assert_eq!(old_persisted.binding(), mismatched_recovery.binding());
        assert_eq!(
            old_persisted.answers().as_slice(),
            &[RevisionAnswer::answered(old_recovered_answer)]
        );

        let live_record = updated_scan
            .records
            .iter()
            .find(|record| record.record.key == live_key)
            .expect("the first live alias checkpoint must remain independent");
        assert_eq!(live_record.record.text, live_text);
        assert_eq!(
            live_record
                .record
                .revision
                .as_ref()
                .expect("the live alias must retain its answer")
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered("Keep the new answer.")]
        );

        assert_ne!(ordinary_key, recovery_key);
        assert_ne!(ordinary_key, live_key);
        let ordinary_record = updated_scan
            .records
            .iter()
            .find(|record| record.record.key == ordinary_key)
            .expect("the edited ordinary tab must use its own recovery key");
        assert_eq!(ordinary_record.record.text, ordinary_live_text);
        assert_eq!(
            ordinary_record
                .record
                .revision
                .as_ref()
                .expect("the ordinary tab must keep its live answer")
                .answers()
                .as_slice(),
            &[RevisionAnswer::answered("Keep the new answer.")]
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .review_flow
                    .recovered_revision_record_for_document(document_id, &ordinary_key)
                    .is_none()
            );
            let ordinary_answers = workspace
                .review_flow
                .revision_recovery_for_document(document_id, &ordinary_key)
                .expect("the ordinary document must keep only its live answer context");
            assert_eq!(
                ordinary_answers.answers().as_slice(),
                &[RevisionAnswer::answered("Keep the new answer.")]
            );
        });
    }

    #[gpui::test]
    fn revision_context_stays_bound_to_one_document_and_cancels_older_work(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.md");
        let second_path = directory.path().join("second.md");
        let first_text = "# First\n";
        let second_text = "# Second\n";
        fs::write(&first_path, first_text).unwrap();
        fs::write(&second_path, second_text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, first_path.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second_path.clone(), window, cx);
            });
        });
        let (
            first_id,
            first_key,
            first_revision,
            first_snapshot,
            second_id,
            second_key,
            second_revision,
            second_snapshot,
        ) = workspace.read_with(cx, |workspace, app| {
            let first = workspace.document_at(0).unwrap().read(app);
            let second = workspace.document_at(1).unwrap().read(app);
            (
                first.id(),
                first.recovery_key(),
                first.revision(),
                first.async_snapshot(app),
                second.id(),
                second.recovery_key(),
                second.revision(),
                second.async_snapshot(app),
            )
        });
        let first_request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Plan,
            Some(&first_path),
            first_text,
            None,
            SourceSnapshot::new(first_revision, first_snapshot.source_generation()),
        )
        .unwrap();
        let second_request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Plan,
            Some(&second_path),
            second_text,
            None,
            SourceSnapshot::new(second_revision, second_snapshot.source_generation()),
        )
        .unwrap();
        let output_for = |scope| mt_core::review::ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope,
            understood_intent: mt_core::review::ReviewSections {
                stated_goal: "preserve the document intent".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "a clearer document".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![mt_core::review::ClarificationQuestion::new(
                "What must remain unchanged?",
                mt_core::review::ClarificationPriority::High,
            )],
        };
        let first_output = output_for(first_request.scope);
        let second_output = output_for(second_request.scope);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.install_revision_context(
                    first_id,
                    first_key,
                    first_snapshot.clone(),
                    first_request,
                    first_output,
                    None,
                    true,
                    window,
                    cx,
                );
                let _ = workspace.review_flow.set_revision_answer_state(
                    0,
                    RevisionAnswer::answered("Keep the first document intent."),
                );
                workspace.open_review_panel(ReviewTarget::Document, cx);
            });
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .document_id,
                first_id
            );
            assert_ne!(
                workspace.review_flow.review_target_document_id(),
                Some(second_id)
            );
        });

        let cancelled = workspace.update(cx, |workspace, _| {
            let _ = workspace
                .review_flow
                .set_revision_answer_state(0, RevisionAnswer::unanswered());
            let (generation, cancelled) = workspace.review_flow.begin_revision_request(first_id);
            assert!(generation > 0);
            cancelled
        });
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.install_revision_context(
                    second_id,
                    second_key,
                    second_snapshot,
                    second_request,
                    second_output,
                    None,
                    true,
                    window,
                    cx,
                );
            });
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(cancelled.load(Ordering::Acquire));
            assert!(!workspace.review_flow.is_revision_running());
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .document_id,
                second_id
            );
        });
    }

    #[gpui_kit::test]
    fn revision_provider_failure_preserves_answer_source_and_editor_until_dismissal(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("revision-provider-failure.md");
        fs::write(&path, "disk source\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let reviewed_text = "edited reviewed source\n";
        replace_document(&workspace, 0, reviewed_text, cx);

        let (document_id, recovery_key, source_snapshot, revision) =
            workspace.read_with(cx, |workspace, app| {
                let document = workspace.document_at(0).unwrap().read(app);
                (
                    document.id(),
                    document.recovery_key(),
                    document.async_snapshot(app),
                    document.revision(),
                )
            });
        let request = build_document_review_request(
            ReviewTarget::Document,
            ArtifactLens::Prompt,
            Some(&path),
            reviewed_text,
            None,
            SourceSnapshot::new(revision, source_snapshot.source_generation()),
        )
        .unwrap();
        let question = ClarificationQuestion::new(
            "What must remain unchanged?",
            ClarificationPriority::Critical,
        );
        let output = ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: ReviewSections {
                stated_goal: "preserve the requested behavior".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "an unchanged source unless approved".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: vec![question],
        };
        let answer_text = "Keep the reviewed source intact.";
        let answer = RevisionAnswer::answered(answer_text);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.install_revision_context(
                    document_id,
                    recovery_key,
                    source_snapshot.clone(),
                    request.clone(),
                    output,
                    None,
                    true,
                    window,
                    cx,
                );
                workspace.set_revision_answer_state(
                    0,
                    RevisionAnswer::answered(String::new()),
                    window,
                    cx,
                );
            });
        });
        let input = workspace.read_with(cx, |workspace, _| {
            workspace
                .review_flow
                .revision_context()
                .unwrap()
                .answer_inputs[0]
                .clone()
        });
        cx.update(|window, app| {
            input.update(app, |input, cx| input.replace_all(answer_text, window, cx));
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            workspace.on_revision_provider_failure(
                mt_core::review::provider::RevisionError::RequestFailed,
                cx,
            );
        });

        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), reviewed_text);
            assert!(document.is_dirty());

            let context = workspace
                .review_flow
                .revision_context()
                .expect("provider failure must retain the reviewed answer context");
            assert_eq!(context.document_id, document_id);
            assert_eq!(context.source_snapshot, source_snapshot);
            assert_eq!(context.request.snapshot, request.snapshot);
            assert_eq!(context.answer_states, vec![answer.clone()]);
            assert_eq!(
                context.answer_inputs[0].read(app).value().to_string(),
                "Keep the reviewed source intact."
            );
            assert!(!workspace.review_flow.is_revision_running());
            assert!(workspace.review_flow.revision_diagnostic().is_some());
            assert!(!workspace.review_flow.has_revision_result());
        });

        workspace.update(cx, |workspace, cx| workspace.dismiss_revision(cx));
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), reviewed_text);
            assert!(document.is_dirty());
            assert!(workspace.review_flow.revision_diagnostic().is_none());
            assert_eq!(
                workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .answer_states,
                vec![answer]
            );
        });
    }

    #[test]
    fn checkpoint_batch_status_keeps_single_signal_messages_and_combines_both() {
        assert_eq!(checkpoint_batch_status(0, None), None);
        assert_eq!(
            checkpoint_batch_status(2, None).as_deref(),
            Some("Recovery skipped 2 malformed, oversized, expired, or unreadable record(s).")
        );
        assert_eq!(
            checkpoint_batch_status(0, Some("recovery encryption failed")).as_deref(),
            Some("recovery encryption failed. Editing and source files are unchanged.")
        );
        assert_eq!(
            checkpoint_batch_status(1, Some("recovery encryption failed")).as_deref(),
            Some(
                "recovery encryption failed. Editing and source files are unchanged. Recovery skipped 1 malformed, oversized, expired, or unreadable record(s)."
            )
        );
    }

    #[gpui_kit::test]
    fn checkpoint_batch_failure_stays_visible_beside_maintenance_issues(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("checkpoint-first.md");
        let second = dir.path().join("checkpoint-second.md");
        fs::write(&first, "first\n").unwrap();
        fs::write(&second, "second\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });

        let now = cx.background_executor.now();
        let issue_path = dir.path().join("malformed.mtrecovery");
        let (first_id, first_key, first_revision, second_id, second_key, second_revision) =
            workspace.read_with(cx, |workspace, app| {
                let first = workspace.document_at(0).unwrap().read(app);
                let second = workspace.document_at(1).unwrap().read(app);
                (
                    first.id(),
                    first.recovery_key(),
                    first.revision(),
                    second.id(),
                    second.recovery_key(),
                    second.revision(),
                )
            });
        let first_token = store.activate_and_current_token(&first_key).0;
        let second_token = store.activate_and_current_token(&second_key).0;
        workspace.update(cx, |workspace, cx| {
            let mut first_schedule = CheckpointSchedule::default();
            first_schedule.mark_dirty(now);
            let mut second_schedule = CheckpointSchedule::default();
            second_schedule.mark_dirty(now);
            let first_attempt = RecoveryAttempt {
                token: first_token.clone(),
                content_identity: RecoveryContentIdentity::for_revision(first_revision),
                timing: first_schedule.checkpoint_dispatched(now).unwrap(),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            let second_attempt = RecoveryAttempt {
                token: second_token.clone(),
                content_identity: RecoveryContentIdentity::for_revision(second_revision),
                timing: second_schedule.checkpoint_dispatched(now).unwrap(),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            workspace.recovery_flow.recovery_schedules.insert(
                first_id,
                DocumentRecoveryState {
                    key: first_key,
                    content_identity: RecoveryContentIdentity::for_revision(first_revision),
                    suppressed_oversized_revision: None,
                    token: Some(first_token),
                    schedule: first_schedule,
                    in_flight: Some(first_attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.recovery_flow.recovery_schedules.insert(
                second_id,
                DocumentRecoveryState {
                    key: second_key,
                    content_identity: RecoveryContentIdentity::for_revision(second_revision),
                    suppressed_oversized_revision: None,
                    token: Some(second_token),
                    schedule: second_schedule,
                    in_flight: Some(second_attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.finish_recovery_checkpoints(
                vec![
                    (first_id, first_attempt, CheckpointBatchOutcome::Written),
                    (
                        second_id,
                        second_attempt,
                        CheckpointBatchOutcome::Failed(RecoveryError::QuotaExceeded {
                            required_records: 51,
                            max_records: 50,
                            required_bytes: 129,
                            max_total_bytes: 128,
                        }),
                    ),
                ],
                RecoveryMaintenance {
                    removed_expired: 0,
                    issues: vec![RecoveryIssue::Malformed { path: issue_path }],
                },
                cx.background_executor().now(),
                cx,
            );
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "recovery retention quota would require 51 records / 129 bytes; limits are 50 records / 128 bytes. Editing and source files are unchanged. Recovery skipped 1 malformed, oversized, expired, or unreadable record(s)."
                )
            );
        });
        assert_eq!(document_text(&workspace, 0, cx), "first\n");
        assert_eq!(document_text(&workspace, 1, cx), "second\n");
        assert_eq!(fs::read_to_string(first).unwrap(), "first\n");
        assert_eq!(fs::read_to_string(second).unwrap(), "second\n");
    }
    #[gpui_kit::test]
    fn multi_document_discard_keeps_every_record_when_batch_retirement_fails(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("batch-retirement-first.md");
        let second = dir.path().join("batch-retirement-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });
        replace_document(&workspace, 0, "first discarded snapshot\n", cx);
        replace_document(&workspace, 1, "second discarded snapshot\n", cx);
        let checkpoints = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_views()
                .into_iter()
                .map(|document| document.read(app).recovery_checkpoint(app))
                .collect::<Vec<_>>()
        });
        for checkpoint in &checkpoints {
            store
                .checkpoint(
                    checkpoint,
                    &checkpoints
                        .iter()
                        .map(|checkpoint| checkpoint.key.clone())
                        .collect(),
                )
                .unwrap();
        }
        store.fail_next_persist_for_test();

        assert!(!cx.simulate_close());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(
                workspace.document_at(0).unwrap().read(app).text(app),
                "first discarded snapshot\n"
            );
            assert_eq!(
                workspace.document_at(1).unwrap().read(app).text(app),
                "second discarded snapshot\n"
            );
            for checkpoint in &checkpoints {
                assert!(
                    workspace
                        .recovery_flow
                        .pending_recovery_retirements
                        .contains_key(&checkpoint.key),
                    "a failed batch marker write must keep every retirement queued"
                );
            }
        });
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|record| (record.record.key, record.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        for checkpoint in checkpoints {
            assert_eq!(recovered.get(&checkpoint.key), Some(&checkpoint.text));
        }
    }

    #[gpui_kit::test]
    fn dirty_owned_old_retirement_delays_the_full_discard_batch(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("old-retirement-first.md");
        let second = dir.path().join("old-retirement-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });
        replace_document(&workspace, 0, "latest first text\n", cx);
        replace_document(&workspace, 1, "latest second text\n", cx);

        let (first_id, checkpoints) = workspace.read_with(cx, |workspace, app| {
            let documents = workspace.document_views();
            let first_id = documents[0].read(app).id();
            let checkpoints = documents
                .into_iter()
                .map(|document| document.read(app).recovery_checkpoint(app))
                .collect::<Vec<_>>();
            (first_id, checkpoints)
        });
        let first_key = checkpoints[0].key.clone();
        let second_key = checkpoints[1].key.clone();
        let active = checkpoints
            .iter()
            .map(|checkpoint| checkpoint.key.clone())
            .collect::<HashSet<_>>();
        for checkpoint in &checkpoints {
            store.checkpoint(checkpoint, &active).unwrap();
        }

        workspace.update(cx, |workspace, cx| {
            workspace.cancel_recovery_attempts_for_key(&first_key, cx.background_executor().now());
        });
        let old_batch = store
            .begin_retirements([first_key.clone()])
            .expect("the old retirement batch");
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_flow
                .recovery_retirement_batches
                .insert(first_key.clone(), old_batch.clone());
        });
        assert!(matches!(
            store.complete_retirements(old_batch.clone()).unwrap(),
            RetirementCompletion::Retired { .. }
        ));

        let (latest_first, token) = workspace.update(cx, |workspace, cx| {
            let document = workspace.document_by_id(first_id, cx).unwrap();
            workspace.arm_document_recovery(&document, cx);
            let checkpoint = document.read(cx).recovery_checkpoint(cx);
            let token = workspace
                .recovery_flow
                .recovery_schedules
                .get(&first_id)
                .and_then(|state| state.token.clone())
                .expect("the first document must have a rearmed token");
            (checkpoint, token)
        });
        assert!(matches!(
            store
                .checkpoint_if_current(&latest_first, &active, token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let documents = workspace.lifecycle_documents(cx);
                let mut request =
                    DestructiveRequest::new(DestructiveAction::CloseWindow, &documents);
                assert!(matches!(
                    request.decide(DirtyDecision::Discard, None, &documents),
                    DestructiveResolution::Prompt(_)
                ));
                let DestructiveResolution::Proceed(action) =
                    request.decide(DirtyDecision::Discard, None, &documents)
                else {
                    panic!("both dirty documents must be authorized for discard");
                };
                workspace.perform_after_discard_retirement(
                    request,
                    action,
                    vec![(first_key.clone(), None), (second_key.clone(), None)],
                    window,
                    cx,
                );
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .get(&first_key),
                Some(&old_batch)
            );
            assert!(
                !workspace
                    .recovery_flow
                    .recovery_retirement_batches
                    .contains_key(&second_key),
                "a dirty-owned old retirement must block a partial B-only batch"
            );
        });
        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);

        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_retirement_batch(
                std::slice::from_ref(&first_key),
                &old_batch,
                cx,
            );
        });
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert!(workspace.read_with(cx, |workspace, _| workspace.window_close_pending));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn saved_document_retirement_does_not_block_later_discard_batch(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("save-then-discard-first.md");
        let second = dir.path().join("save-then-discard-second.md");
        fs::write(&first, "first disk\n").unwrap();
        fs::write(&second, "second disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first.clone()), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second.clone(), window, cx);
            });
        });
        replace_document(&workspace, 0, "first saved text\n", cx);
        replace_document(&workspace, 1, "second discarded text\n", cx);
        let checkpoints = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_views()
                .into_iter()
                .map(|document| document.read(app).recovery_checkpoint(app))
                .collect::<Vec<_>>()
        });
        let active = checkpoints
            .iter()
            .map(|checkpoint| checkpoint.key.clone())
            .collect::<HashSet<_>>();
        for checkpoint in &checkpoints {
            store.checkpoint(checkpoint, &active).unwrap();
        }
        store.fail_next_delete_for_test();

        assert!(!cx.simulate_close());
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .recovery_flow
                    .recovery_retirements
                    .contains_key(&RecoveryKey::for_path(&first)),
                "the saved document must retain its durable single-key retirement while cleanup retries"
            );
        });

        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert!(workspace.read_with(cx, |workspace, _| workspace.window_close_pending));
        assert_eq!(fs::read_to_string(first).unwrap(), "first saved text\n");
        assert_eq!(fs::read_to_string(second).unwrap(), "second disk\n");
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui_kit::test]
    fn window_close_walks_multiple_dirty_documents(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.md");
        let second = dir.path().join("second.md");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, first);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });
        replace_document(&workspace, 0, "dirty one\n", cx);
        replace_document(&workspace, 1, "dirty two\n", cx);

        assert!(!cx.simulate_close());
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(
            cx.has_pending_prompt(),
            "the second dirty document must prompt"
        );
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert!(workspace.read_with(cx, |workspace, _| workspace.window_close_pending));
    }

    #[gpui_kit::test]
    fn cancelling_a_multi_document_close_keeps_all_recovery_records(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first-recovery.md");
        let second = dir.path().join("second-recovery.md");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) =
            open_test_workspace_with_recovery_store(cx, Some(first), store.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });
        replace_document(&workspace, 0, "dirty one\n", cx);
        replace_document(&workspace, 1, "dirty two\n", cx);
        let checkpoints = workspace.read_with(cx, |workspace, app| {
            workspace
                .document_views()
                .into_iter()
                .map(|document| document.read(app).recovery_checkpoint(app))
                .collect::<Vec<_>>()
        });
        let active = checkpoints
            .iter()
            .map(|checkpoint| checkpoint.key.clone())
            .collect::<HashSet<_>>();
        for checkpoint in &checkpoints {
            store.checkpoint(checkpoint, &active).unwrap();
        }

        assert!(!cx.simulate_close());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert_eq!(
            store.recover().unwrap().records.len(),
            2,
            "a cancelled action has not intentionally discarded either open dirty buffer"
        );
    }

    #[gpui_kit::test]
    fn window_close_rechecks_documents_that_become_dirty_during_a_prompt(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first-dirty.md");
        let second = dir.path().join("becomes-dirty.md");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, first);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });
        replace_document(&workspace, 0, "dirty one\n", cx);

        assert!(!cx.simulate_close());
        assert!(cx.has_pending_prompt());
        replace_document(&workspace, 1, "new async text\n", cx);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "the newly dirty document must be checked before the window can close"
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(cx.cx.update(|app| app.windows().len()), 1);
        assert_eq!(document_text(&workspace, 1, cx), "new async text\n");
    }

    #[gpui_kit::test]
    fn dirty_close_reprompts_when_the_prompted_document_changes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompted-document.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        replace_document(&workspace, 0, "first draft\n", cx);

        cx.simulate_keystrokes("ctrl-w");
        assert!(cx.has_pending_prompt());
        replace_document(&workspace, 0, "newer draft\n", cx);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "an answer for the first revision cannot discard a newer revision"
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            1
        );
        assert_eq!(document_text(&workspace, 0, cx), "newer draft\n");
    }

    #[gpui_kit::test]
    fn workspace_replace_rechecks_documents_that_become_dirty_during_a_prompt(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let first = dir.path().join("first-dirty.md");
        let second = dir.path().join("becomes-dirty.md");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, first);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(second, window, cx);
            });
        });
        replace_document(&workspace, 0, "dirty one\n", cx);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_workspace_replace(replacement.path().to_path_buf(), window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        replace_document(&workspace, 1, "new async text\n", cx);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "the newly dirty document must be checked before replacing the workspace"
        );
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.tabs.len()),
            2
        );
        assert_eq!(document_text(&workspace, 1, cx), "new async text\n");
    }

    #[gpui_kit::test]
    fn workspace_replace_save_persists_text_before_switching_roots(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let path = dir.path().join("replace-save.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "saved before replace 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_workspace_replace(replacement.path().to_path_buf(), window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.root.as_deref(), Some(replacement.path()));
            assert!(workspace.tabs.is_empty());
        });
        assert_eq!(fs::read_to_string(path).unwrap(), edited);
    }

    #[gpui_kit::test]
    fn workspace_replace_discard_switches_roots_without_writing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let path = dir.path().join("replace-discard.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        replace_document(&workspace, 0, "editor only\n", cx);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_workspace_replace(replacement.path().to_path_buf(), window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.root.as_deref(), Some(replacement.path()));
            assert!(workspace.tabs.is_empty());
        });
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
    }

    #[gpui_kit::test]
    fn workspace_replace_cancel_preserves_root_tab_and_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let path = dir.path().join("replace-cancel.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "keep current workspace 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.request_workspace_replace(replacement.path().to_path_buf(), window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.root.as_deref(), Some(dir.path()));
            assert_eq!(workspace.tabs.len(), 1);
        });
        assert_eq!(document_text(&workspace, 0, cx), edited);
        assert_eq!(fs::read_to_string(path).unwrap(), "disk\n");
    }

    #[gpui_kit::test]
    fn removed_watcher_event_preserves_dirty_text_until_recreate_or_save_as(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("removed.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "editor survives remove 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::remove_file(&path).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(dir.path(), &[Change::Removed(path.clone())], cx);
        });

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), edited);
        });

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert!(!path.exists(), "Ctrl-S must not recreate a removed source");
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            true,
            "The source path no longer exists. Recreate it or Save As.",
            cx,
        );
    }

    #[gpui_kit::test]
    fn rename_shaped_watcher_events_preserve_dirty_text_until_recreate_or_save_as(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rename-source.md");
        let renamed = dir.path().join("rename-destination.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        let edited = "editor survives rename 中文 \u{1f680}\n";
        replace_document(&workspace, 0, edited, cx);
        fs::rename(&path, &renamed).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(
                dir.path(),
                &[
                    Change::Removed(path.clone()),
                    Change::Created(renamed.clone()),
                ],
                cx,
            );
        });

        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), edited);
        });

        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();

        assert!(
            !path.exists(),
            "Ctrl-S must not recreate the old renamed path"
        );
        assert_eq!(fs::read_to_string(&renamed).unwrap(), "disk\n");
        assert_failed_save_preserves_document(
            &workspace,
            edited,
            true,
            "The source path no longer exists. Recreate it or Save As.",
            cx,
        );
    }
    #[gpui_kit::test]
    fn recovered_document_retains_text_metadata_and_conflict_state(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.txt");
        fs::write(&path, b"\xFF\xFEh\x00i\x00\r\x00\n\x00").unwrap();
        let loaded = mt_core::document::io::load(&path).unwrap();
        let metadata = RecoveryMetadata::from_loaded_file(&loaded);
        let original_stamp = metadata.original_stamp.clone();
        let recovered_text = "recovered 中文 \u{1f680}\n";
        let scan = RecoveryScan {
            records: vec![RecoveredRecord {
                record: RecoveryRecord {
                    key: RecoveryKey::for_path(&path),
                    text: recovered_text.into(),
                    metadata,
                    checkpointed_at: SystemTime::now(),
                    revision: None,
                },
                source_conflicted: true,
            }],
            issues: Vec::new(),
        };
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace_with_recovery_store(cx, None, store);
        let restored = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_recovery_for_test(workspace, scan, window, cx)
            })
        });

        assert_eq!(restored, (1, 0));
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let id = document.read_with(cx, |document, _| document.id());
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_flow.recovery_schedules.contains_key(&id));
            assert!(workspace.recovery_flow._recovery_timer.is_some());
        });
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), recovered_text);
            let checkpoint = document.recovery_checkpoint(app);
            assert_eq!(checkpoint.metadata.encoding_name, "UTF-16LE");
            assert!(checkpoint.metadata.had_bom);
            assert_eq!(
                checkpoint.metadata.newline,
                mt_core::document::io::Newline::Crlf
            );
            assert_eq!(checkpoint.metadata.original_stamp, original_stamp);
        });
    }

    #[test]
    fn workspace_panel_widths_preserve_preferences_and_the_document_floor() {
        let widths = resolved_workspace_panel_widths(
            gpui_kit::px(224.),
            gpui_kit::px(288.),
            true,
            true,
            gpui_kit::px(1200.),
        );

        assert_eq!(widths.left, gpui_kit::px(224.));
        assert_eq!(widths.right, gpui_kit::px(288.));

        let collapsed = resolved_workspace_panel_widths(
            gpui_kit::px(224.),
            gpui_kit::px(288.),
            false,
            false,
            gpui_kit::px(1200.),
        );
        assert_eq!(collapsed.left, gpui_kit::px(0.));
        assert_eq!(collapsed.right, gpui_kit::px(0.));

        let restored = resolved_workspace_panel_widths(
            gpui_kit::px(224.),
            gpui_kit::px(288.),
            true,
            true,
            gpui_kit::px(1200.),
        );
        assert_eq!(restored.left, gpui_kit::px(224.));
        assert_eq!(restored.right, gpui_kit::px(288.));

        let narrowed = resolved_workspace_panel_widths(
            gpui_kit::px(640.),
            gpui_kit::px(720.),
            true,
            true,
            gpui_kit::px(720.),
        );
        assert_eq!(
            narrowed.left + narrowed.right,
            gpui_kit::px(440.),
            "restoring both panels must leave the document its 280px \
             useful-width floor"
        );
        assert_eq!(narrowed.left, gpui_kit::px(crate::metrics::SIDE_PANEL.min));
        assert_eq!(
            narrowed.right,
            gpui_kit::px(crate::metrics::RIGHT_PANEL.min)
        );

        let left_only = resolved_workspace_panel_widths(
            gpui_kit::px(640.),
            gpui_kit::px(288.),
            true,
            false,
            gpui_kit::px(720.),
        );
        assert_eq!(left_only.left, gpui_kit::px(440.));

        for (viewport, expected_side_budget) in [(600., 320.), (300., 20.)] {
            let forced = resolved_workspace_panel_widths(
                gpui_kit::px(640.),
                gpui_kit::px(720.),
                true,
                true,
                gpui_kit::px(viewport),
            );
            assert_eq!(
                forced.left + forced.right,
                gpui_kit::px(expected_side_budget),
                "forced viewport {viewport}px must preserve the document budget"
            );
        }

        let tiny_right = resolved_workspace_panel_widths(
            gpui_kit::px(224.),
            gpui_kit::px(720.),
            false,
            true,
            gpui_kit::px(300.),
        );
        assert_eq!(tiny_right.right, gpui_kit::px(20.));
    }

    #[test]
    fn details_content_follows_the_visible_context() {
        assert_eq!(
            details_content(SidePanel::Files, false, false, false),
            DetailsContent::Empty
        );
        assert_eq!(
            details_content(SidePanel::Files, false, true, false),
            DetailsContent::Document
        );
        assert_eq!(
            details_content(SidePanel::Harness, false, true, true),
            DetailsContent::Harness
        );
        assert_eq!(
            details_content(SidePanel::Files, false, true, true),
            DetailsContent::Document
        );
        assert_eq!(
            details_content(SidePanel::Harness, false, false, true),
            DetailsContent::Harness
        );
        assert_eq!(
            details_content(SidePanel::Harness, true, true, true),
            DetailsContent::Empty
        );
    }

    #[test]
    fn panel_drag_clamps_only_the_dragged_preference() {
        assert_eq!(
            clamped_dragged_panel_width(
                WorkspaceResizeEdge::Left,
                gpui_kit::px(900.),
                gpui_kit::px(288.),
                true,
                gpui_kit::px(1200.),
            ),
            gpui_kit::px(632.),
            "the opposite panel and document floor bound the dragged side"
        );
        assert_eq!(
            clamped_dragged_panel_width(
                WorkspaceResizeEdge::Right,
                gpui_kit::px(40.),
                gpui_kit::px(224.),
                true,
                gpui_kit::px(1200.),
            ),
            gpui_kit::px(crate::metrics::RIGHT_PANEL.min),
            "normal viewports retain the panel's useful minimum"
        );
        assert_eq!(
            clamped_dragged_panel_width(
                WorkspaceResizeEdge::Right,
                gpui_kit::px(720.),
                gpui_kit::px(180.),
                true,
                gpui_kit::px(300.),
            ),
            gpui_kit::px(0.),
            "a forced tiny viewport yields to the document rather than panicking"
        );
    }

    #[gpui_kit::test]
    fn dragging_the_workspace_divider_updates_the_owned_column(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::settings::AppSettings::init(cx);
            crate::settings::AppSettings::update(cx, |settings| {
                settings.show_welcome_on_startup = false;
            });
        });
        let captured = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(None, window, cx));
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_kit::component::Root::new(workspace, window, cx)
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let workspace = captured.borrow().clone().expect("the Workspace entity");
        let before = workspace.read_with(cx, |workspace, _| workspace.preferred_left_panel_width);
        let handle = cx
            .debug_bounds("left-panel-resize-handle")
            .expect("the left resize handle");
        assert_eq!(
            handle.origin.x + px(4.),
            before,
            "the splitter line must sit on the owned panel boundary"
        );
        let start = point(
            handle.origin.x + handle.size.width / 2.,
            handle.origin.y + handle.size.height / 2.,
        );

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            point(start.x + px(10.), start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(start.x + px(60.), start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(start.x + px(60.), start.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let (after, grab_cleared) = workspace.read_with(cx, |workspace, _| {
            (
                workspace.preferred_left_panel_width,
                workspace.panel_resize_grab.is_none(),
            )
        });
        assert!(after > before, "dragging right must widen the owned column");
        assert!(
            grab_cleared,
            "the resize gesture must release retained state"
        );
        let column = cx
            .debug_bounds("left-workspace-column")
            .expect("the resolved left workspace column");
        assert_eq!(column.size.width, after);
    }

    #[gpui_kit::test]
    fn keyboard_resizes_the_focused_workspace_divider(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::settings::AppSettings::init(cx);
            crate::settings::AppSettings::update(cx, |settings| {
                settings.show_welcome_on_startup = false;
            });
        });
        let captured = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(None, window, cx));
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_kit::component::Root::new(workspace, window, cx)
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let workspace = captured.borrow().clone().expect("the Workspace entity");
        let handle = cx
            .debug_bounds("left-panel-resize-handle")
            .expect("the left resize handle");
        let position = point(
            handle.origin.x + handle.size.width / 2.,
            handle.origin.y + handle.size.height / 2.,
        );
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        let before = workspace.read_with(cx, |workspace, _| workspace.preferred_left_panel_width);

        cx.simulate_keystrokes("right");
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let after = workspace.read_with(cx, |workspace, _| workspace.preferred_left_panel_width);
        assert_eq!(after, before + crate::metrics::gap_group());
    }

    #[test]
    fn short_names_are_left_alone() {
        for name in ["a.md", "README.md", "architecture.md"] {
            assert_eq!(elide_tab_label(name), name);
        }
    }

    #[test]
    fn only_harness_discovery_paths_trigger_a_rescan() {
        let root = Path::new("workspace");

        for unrelated in [
            "src/widget.rs",
            "src/rules_engine.rs",
            "docs/skill-design.md",
            ".github/workflows/build.yml",
            ".claude/settings.json",
            ".agents/skills/review/references/notes.md",
            ".agents/skills/review/scripts/run.py",
            "rules/engine.rs",
            "instructions/data.json",
            "memories/cache.txt",
            "rules/category/deep/ignored.mdc",
            "notes.tmp",
        ] {
            assert!(!path_affects_harness(root, &root.join(unrelated)));
        }

        for relevant in [
            "AGENTS.md",
            "GEMINI.md",
            "QWEN.md",
            "AGENT.md",
            "rules/AGENT.md",
            "rules/category",
            "rules/category/agent.instructions.md",
            ".cursor/rules/project.mdc",
            ".cursor/rules/category",
            ".github/instructions/rust.instructions.md",
            ".claude/GEMINI.md",
            ".agents/skills/review/SKILL.md",
            ".agents/skills/review",
            ".agents/skills/review/references",
            "skills/new-skill",
        ] {
            assert!(
                path_affects_harness(root, &root.join(relevant)),
                "{relevant}"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        for relevant in ["skills/review.v2", ".cursor/rules/team.v2"] {
            let path = dir.path().join(relevant);
            std::fs::create_dir_all(&path).unwrap();
            assert!(path_affects_harness(dir.path(), &path), "{relevant}");
        }
    }

    #[test]
    fn long_names_keep_their_extension() {
        // Eliding from the end is the obvious implementation and removes
        // exactly the part worth keeping: `notes.md` and `notes.mdx` are
        // different documents, and the extension is what says which.
        let long = "a-very-long-document-name-indeed.mdx";
        let out = elide_tab_label(long);
        assert!(out.ends_with(".mdx"), "got {out}");
        assert!(out.contains('…'), "got {out}");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
    }

    #[test]
    fn elision_counts_characters_not_bytes() {
        // A CJK name is well under the limit in characters and well over it in
        // bytes; slicing by byte would also panic mid-codepoint.
        let name = "这是一个很长的中文文档名称.md";
        let out = elide_tab_label(name);
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        // Long enough to survive intact at this limit.
        assert_eq!(out, name);

        let longer = "这是一个非常非常非常长的中文文档名称需要省略.md";
        let out = elide_tab_label(longer);
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        assert!(out.ends_with(".md"), "got {out}");
    }

    #[test]
    fn a_dotfile_is_not_all_extension() {
        // `.gitignore` has no stem before the dot; treating the whole name as
        // an extension would leave nothing to elide and produce just "…".
        let out = elide_tab_label(".a-really-long-dotfile-name-here");
        assert!(!out.starts_with('…'), "got {out}");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
    }

    #[test]
    fn a_name_with_no_extension_still_elides() {
        let out = elide_tab_label("LICENSE-WITH-A-VERY-LONG-SUFFIX");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        assert!(out.ends_with('…'), "got {out}");
    }

    #[test]
    fn document_details_status_prioritizes_external_changes() {
        assert_eq!(
            document_details_status_key(false, false),
            i18n::Key::Saved,
            "a clean document whose file matches disk is saved"
        );
        assert_eq!(
            document_details_status_key(false, true),
            i18n::Key::UnsavedChanges,
            "local edits are unsaved when no external change exists"
        );
        assert_eq!(
            document_details_status_key(true, false),
            i18n::Key::ChangedOnDisk,
            "an external change is not saved even without local edits"
        );
        assert_eq!(
            document_details_status_key(true, true),
            i18n::Key::ChangedOnDisk,
            "an external conflict takes precedence over local unsaved edits"
        );
    }

    #[gpui::test]
    fn revision_skill_supporting_changes_remain_stale_after_save(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let support_dir = directory.path().join("references");
        let support = support_dir.join("guide.md");
        fs::create_dir(&support_dir).unwrap();
        fs::write(&skill, "# Skill\n").unwrap();
        fs::write(&support, "original support\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(support.clone(), window, cx);
            });
        });
        let (document_id, source_snapshot, revision, skill_origin) =
            workspace.read_with(cx, |workspace, app| {
                let document = workspace.document_at(0).unwrap().read(app);
                (
                    document.id(),
                    document.async_snapshot(app),
                    document.revision(),
                    document
                        .skill_origin()
                        .expect("the opened Skill entrypoint has verified provenance")
                        .clone(),
                )
            });
        let (request, package) = super::build_review_request(
            super::ReviewRequestBuildRequest::new(
                ReviewTarget::Document,
                ArtifactLens::AgentSkill,
                Some(&skill),
                "# Skill\n",
                None,
                SourceSnapshot::new(revision, source_snapshot.source_generation()),
                false,
            )
            .with_skill_origin(Some(&skill_origin)),
        )
        .unwrap()
        .into_parts();
        let package = package.expect("Agent Skill Review owns its frozen package");
        assert!(package.path_affects_root(&package.root().join("references/new-guide.md")));
        let output = mt_core::review::ReviewModelOutput {
            schema_version: mt_core::review::REVIEW_SCHEMA_VERSION.to_owned(),
            scope: request.scope,
            understood_intent: mt_core::review::ReviewSections {
                stated_goal: "preserve the skill".into(),
                relevant_context: Vec::new(),
                constraints: Vec::new(),
                non_goals: Vec::new(),
                expected_deliverable: "a reviewed skill".into(),
                success_evidence: Vec::new(),
                inferred_assumptions: Vec::new(),
                unresolved_decisions: Vec::new(),
            },
            findings: Vec::new(),
            clarification_questions: Vec::new(),
        };
        workspace.update(cx, |workspace, _| {
            workspace
                .review_flow
                .install_revision_context(super::WorkspaceRevisionContext {
                    document_id,
                    source_snapshot,
                    request,
                    review_output: output,
                    skill_package: Some(package.clone()),
                    supporting_sources_current: true,
                    applied: None,
                    answers_exported: false,
                    answer_states: Vec::new(),
                    answer_inputs: Vec::new(),
                });
        });

        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(
                directory.path(),
                &[Change::Modified(support.clone())],
                cx,
            );
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .supporting_sources_current
            );
        });

        let new_support = support_dir.join("new-guide.md");
        fs::write(&new_support, "new support\n").unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .review_flow
                .set_revision_supporting_sources_current(true);
        });
        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(
                directory.path(),
                &[Change::Created(new_support.clone())],
                cx,
            );
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .supporting_sources_current
            );
        });

        fs::remove_file(&new_support).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .review_flow
                .set_revision_supporting_sources_current(true);
        });
        workspace.update(cx, |workspace, cx| {
            workspace.apply_watcher_changes(directory.path(), &[Change::Removed(new_support)], cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .supporting_sources_current
            );
        });

        workspace.update(cx, |workspace, _| {
            workspace
                .review_flow
                .set_revision_supporting_sources_current(true);
        });
        replace_document(&workspace, 1, "changed support\n", cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .supporting_sources_current
            );
        });
        let support_document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(1).cloned())
            .unwrap();
        support_document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        assert!(package.revalidate().is_err());
        workspace.read_with(cx, |workspace, _| {
            assert!(
                !workspace
                    .review_flow
                    .revision_context()
                    .unwrap()
                    .supporting_sources_current
            );
        });
    }

    #[gpui_kit::test]
    fn stable_skill_review_uses_its_verified_loaded_origin(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "# Stable skill\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill.clone());

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let document_id = workspace.active_document().unwrap().read(cx).id();
                workspace
                    .review_flow
                    .set_review_lens_override(document_id, ArtifactLens::AgentSkill);
                workspace.review(ReviewTarget::Document, window, cx);
            });
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            let document = workspace.active_document().unwrap().read(app);
            assert!(document.skill_origin().is_some());
            assert_eq!(document.source_path(), Some(skill.as_path()));
            assert_eq!(document.text(app), "# Stable skill\n");
            assert!(!document.is_dirty());

            let diagnostic = workspace
                .review_flow
                .review_diagnostic()
                .expect("request preparation reaches the unavailable-provider result");
            assert_eq!(diagnostic.lens, ArtifactLens::AgentSkill);
            assert!(matches!(
                diagnostic.diagnostic.code,
                ReviewDiagnosticCode::NoProvider | ReviewDiagnosticCode::Unavailable
            ));
            assert!(!workspace.review_flow.is_reviewing());
        });
        assert!(!cx.has_pending_prompt());
    }

    #[gpui_kit::test]
    fn substituted_skill_root_is_rejected_before_consent(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill_root = directory.path().join("skill");
        let displaced_root = directory.path().join("displaced-skill");
        fs::create_dir(&skill_root).unwrap();
        let skill = skill_root.join("SKILL.md");
        fs::write(&skill, "# Original skill\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill.clone());

        fs::rename(&skill_root, &displaced_root).unwrap();
        fs::create_dir(&skill_root).unwrap();
        fs::write(&skill, "# Substitute skill\n").unwrap();
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let document_id = workspace.active_document().unwrap().read(cx).id();
                workspace
                    .review_flow
                    .set_review_lens_override(document_id, ArtifactLens::AgentSkill);
                workspace.review(ReviewTarget::Document, window, cx);
            });
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            let document = workspace.active_document().unwrap().read(app);
            assert_eq!(document.text(app), "# Original skill\n");
            assert!(!document.is_dirty());

            let diagnostic = workspace
                .review_flow
                .review_diagnostic()
                .expect("a substituted root fails during request preparation");
            assert_eq!(diagnostic.lens, ArtifactLens::AgentSkill);
            assert_eq!(
                diagnostic.diagnostic.code,
                ReviewDiagnosticCode::InvalidRequest
            );
            assert_eq!(
                diagnostic.diagnostic.message.as_str(),
                i18n::t(i18n::Key::ReviewSkillPackageChanged, app)
            );
            assert!(!workspace.review_flow.is_reviewing());
        });
        assert!(!cx.has_pending_prompt());
    }

    #[gpui_kit::test]
    fn recovered_skill_without_origin_remains_editable_and_save_as_works(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let save_as = directory.path().join("recovered-copy.md");
        fs::write(&skill, "# Disk skill\n").unwrap();
        let recovered_text = "# Recovered skill text\n";
        let recovered = RecoveredRecord {
            record: RecoveryRecord {
                key: RecoveryKey::for_path(&skill),
                text: recovered_text.to_owned(),
                metadata: RecoveryMetadata {
                    source_path: Some(skill.clone()),
                    encoding_name: "UTF-8".to_owned(),
                    had_bom: false,
                    newline: Newline::Lf,
                    original_stamp: FileStamp::of(&skill).unwrap(),
                    source_identity: SourceIdentity::Regular,
                    decode_had_errors: false,
                },
                checkpointed_at: SystemTime::now(),
                revision: None,
            },
            source_conflicted: false,
        };
        let prepared = DocumentView::prepare_recovery(recovered).unwrap();
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let document = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                let registry = workspace.registry.clone();
                let document =
                    cx.new(|cx| DocumentView::from_recovery(prepared, registry, window, cx));
                workspace.insert_document(skill.clone(), document.clone(), window, cx);
                let document_id = document.read(cx).id();
                workspace
                    .review_flow
                    .set_review_lens_override(document_id, ArtifactLens::AgentSkill);
                workspace.review(ReviewTarget::Document, window, cx);
                document
            })
        });
        cx.run_until_parked();

        document.read_with(cx, |document, app| {
            assert!(document.skill_origin().is_none());
            assert_eq!(document.text(app), recovered_text);
            assert!(document.is_dirty());
        });
        workspace.read_with(cx, |workspace, app| {
            let diagnostic = workspace
                .review_flow
                .review_diagnostic()
                .expect("recovered Skill Review must fail closed");
            assert_eq!(
                diagnostic.diagnostic.code,
                ReviewDiagnosticCode::InvalidRequest
            );
            assert_eq!(
                diagnostic.diagnostic.message.as_str(),
                i18n::t(i18n::Key::ReviewSkillPackageUnavailable, app)
            );
            assert!(!workspace.review_flow.is_reviewing());
        });
        assert!(!cx.has_pending_prompt());

        let document_id = document.read_with(cx, |document, _| document.id());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(
                    document_id,
                    save_as.clone(),
                    SaveAsMode::CreateOnly,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        assert_eq!(fs::read(&save_as).unwrap(), recovered_text.as_bytes());
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.tabs.active().and_then(|tab| tab.path()),
                Some(save_as.as_path())
            );
        });
        document.read_with(cx, |document, app| {
            assert_eq!(document.source_path(), Some(save_as.as_path()));
            assert_eq!(document.text(app), recovered_text);
            assert!(!document.is_dirty());
        });
    }

    #[gpui_kit::test]
    fn dirty_skill_entrypoint_anchor_keeps_the_matching_editor_tab(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(&skill, "# Disk entrypoint\n").unwrap();
        let lexical_skill = nested.join("..").join("SKILL.md");
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let document = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_file(lexical_skill.clone(), window, cx));
                workspace.active_document().unwrap().clone()
            })
        });
        let editor_text = "# Dirty editor-only entrypoint\n";
        cx.update(|window, app| {
            document.update(app, |document, cx| {
                document.replace_text(editor_text.to_owned(), window, cx);
            });
        });
        cx.run_until_parked();
        let anchor_start = editor_text.find("editor-only").unwrap();
        let anchor = install_frozen_skill_review(
            &workspace,
            "SKILL.md",
            SourceLocation::bytes(
                anchor_start as u64,
                (anchor_start + "editor-only".len()) as u64,
            ),
            cx,
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace
                    .review_flow
                    .review_result()
                    .unwrap()
                    .skill_package
                    .as_ref()
                    .unwrap()
                    .entrypoint_is_editor_text()
            );
        });

        // The editor-only package is independent of the current disk entrypoint.
        fs::write(&skill, "# Replaced on disk\n").unwrap();
        let document_id = document.read_with(cx, |document, _| document.id());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.reveal_review_anchor(&anchor, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 1);
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), document_id);
            assert_eq!(active.source_path(), Some(lexical_skill.as_path()));
            assert_eq!(active.text(app), editor_text);
            assert!(active.is_dirty());
            assert_eq!(active.cursor(app), anchor_start);
        });
    }

    #[gpui_kit::test]
    fn skill_anchor_opens_a_supporting_file_from_the_frozen_snapshot(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let support = directory.path().join("references").join("guide.md");
        fs::create_dir_all(support.parent().unwrap()).unwrap();
        fs::write(&skill, "# Skill\n").unwrap();
        let support_text = "# Frozen supporting bytes\n";
        fs::write(&support, support_text).unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill);
        let anchor_start = support_text.find("supporting").unwrap();
        let anchor = install_frozen_skill_review(
            &workspace,
            "references/guide.md",
            SourceLocation::bytes(
                anchor_start as u64,
                (anchor_start + "supporting".len()) as u64,
            ),
            cx,
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.reveal_review_anchor(&anchor, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let active = workspace.active_document().unwrap().read(app);
            assert!(
                active
                    .source_path()
                    .is_some_and(|path| mt_core::document::io::paths_match(path, &support))
            );
            assert_eq!(active.text(app), support_text);
            assert_eq!(active.cursor(app), anchor_start);
            assert!(!active.is_dirty());
            assert!(
                workspace
                    .tabs
                    .preview()
                    .is_some_and(|path| mt_core::document::io::paths_match(path, &support))
            );
        });
    }

    #[gpui_kit::test]
    fn skill_anchor_refuses_an_existing_clean_tab_with_other_bytes(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let support = directory.path().join("references").join("guide.md");
        fs::create_dir_all(support.parent().unwrap()).unwrap();
        fs::write(&skill, "# Skill\n").unwrap();
        fs::write(&support, "# Old open-tab bytes\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_file(support.clone(), window, cx));
                assert!(workspace.open_file(skill.clone(), window, cx));
            });
        });
        let support_text = "# Current frozen bytes\n";
        fs::write(&support, support_text).unwrap();
        let anchor_start = support_text.find("frozen").unwrap();
        let anchor = install_frozen_skill_review(
            &workspace,
            "references/guide.md",
            SourceLocation::bytes(anchor_start as u64, (anchor_start + "frozen".len()) as u64),
            cx,
        );
        let skill_document_id = workspace.read_with(cx, |workspace, app| {
            workspace.active_document().unwrap().read(app).id()
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.reveal_review_anchor(&anchor, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            let active = workspace.active_document().unwrap().read(app);
            assert_eq!(active.id(), skill_document_id);
            assert_eq!(active.source_path(), Some(skill.as_path()));
            let old_support = workspace
                .document_at(1)
                .expect("the original support tab remains open")
                .read(app);
            assert_eq!(old_support.text(app), "# Old open-tab bytes\n");
            assert_eq!(
                workspace.status.as_deref(),
                Some(i18n::t(i18n::Key::ReviewStale, app))
            );
        });
    }

    #[gpui_kit::test]
    fn skill_anchor_preserves_a_dirty_supporting_tab_and_marks_review_stale(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let support = directory.path().join("references").join("guide.md");
        fs::create_dir_all(support.parent().unwrap()).unwrap();
        fs::write(&skill, "# Skill\n").unwrap();
        let disk_support = "# Disk supporting text\n";
        fs::write(&support, disk_support).unwrap();
        let (workspace, cx) = open_test_workspace(cx, skill.clone());
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_file(support.clone(), window, cx));
                assert!(workspace.open_file(skill.clone(), window, cx));
            });
        });
        let dirty_support = "# Unsaved supporting edit\n";
        replace_document(&workspace, 1, dirty_support, cx);
        let anchor_start = disk_support.find("supporting").unwrap();
        let anchor = install_frozen_skill_review(
            &workspace,
            "references/guide.md",
            SourceLocation::bytes(
                anchor_start as u64,
                (anchor_start + "supporting".len()) as u64,
            ),
            cx,
        );
        let skill_document_id = workspace.read_with(cx, |workspace, app| {
            workspace.active_document().unwrap().read(app).id()
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.reveal_review_anchor(&anchor, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.tabs.len(), 2);
            assert_eq!(
                workspace.active_document().unwrap().read(app).id(),
                skill_document_id
            );
            let support_document = workspace.document_at(1).unwrap().read(app);
            assert_eq!(support_document.text(app), dirty_support);
            assert!(support_document.is_dirty());
            let review = workspace.review_flow.review_result().unwrap();
            assert!(!review.supporting_sources_current);
            assert!(review.result.result.status.is_stale());
        });
    }

    #[test]
    fn review_lens_correction_cycles_through_all_artifact_choices() {
        assert_eq!(
            next_review_lens(ArtifactLens::Prompt),
            ArtifactLens::Specification
        );
        assert_eq!(
            next_review_lens(ArtifactLens::Specification),
            ArtifactLens::Plan
        );
        assert_eq!(
            next_review_lens(ArtifactLens::Plan),
            ArtifactLens::AgentInstructions
        );
        assert_eq!(
            next_review_lens(ArtifactLens::AgentInstructions),
            ArtifactLens::AgentSkill
        );
        assert_eq!(
            next_review_lens(ArtifactLens::AgentSkill),
            ArtifactLens::Prompt
        );
    }
}
