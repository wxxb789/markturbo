//! The workspace: explorer + skills + outline on the left, tabbed documents on
//! the right.
//!
//! Owns the open-document set, the filesystem watcher, the Web preview surface,
//! and the commands (open folder, save, translate). Individual views stay
//! narrow; this is where they are wired together.
//!
//! Two clusters live in submodules because neither belongs to the wiring.
//! `history` is plain data plus the two buttons that read it; `web_surface`
//! is the OS child window and the re-entrancy rules for touching it. Both add
//! their methods to `Workspace` from there, so `self.web_dirty(cx)` reads the
//! same here as it did before the split.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::{Button as BaseButton, GlobalState, Toggle as BaseToggle};
use gpui_component::{
    ActiveTheme as _, Disableable as _, ElementExt as _, Icon, IconName, Sizable as _,
    StyledExt as _, TITLE_BAR_HEIGHT as COMPONENT_TITLE_BAR_HEIGHT, ThemeStyled as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    menu::ContextMenuExt as _,
    spinner::Spinner,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use mt_doc::translate::Scope;

use crate::fs;
use crate::i18n;
use crate::lifecycle::{
    DestructiveAction, DestructiveRequest, DestructiveResolution, DirtyDecision, DocumentId,
    DocumentLifecycle,
};
use crate::metrics;
use crate::recovery::{
    CancellableRecoveryCheckpointAttempt, CheckpointAttemptTiming, CheckpointBatchOutcome,
    CheckpointSchedule, RecoveredRecord, RecoveryError, RecoveryKey, RecoveryMaintenance,
    RecoveryRetirement, RecoveryRetirementBatch, RecoveryStore, RecoveryToken,
    RetirementCompletion,
};
use crate::renderer::RendererRegistry;
use crate::translate::Provider;
use crate::views::document::{
    DocumentEvent, DocumentView, PreparedRecovery, SaveAsMode, SaveAsOutcome, SaveMode, paths_match,
};
use crate::views::explorer::{Explorer, ExplorerEvent};
use crate::views::harness::{HarnessEvent, HarnessView};
use crate::views::search::{Corpus, SearchEvent, SearchView};
use crate::views::settings_page::{SettingsEvent, SettingsView};
use crate::views::tabs::{TabIdentity, Tabs};
use crate::watcher::{Change, Watcher};

mod history;
pub(crate) mod web_surface;

use self::history::History;
use self::web_surface::WebSurface;

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
const WELCOME_NEW_ACCESSIBILITY_ID: &str = "markturbo-welcome-new";
const WELCOME_PASTE_ACCESSIBILITY_ID: &str = "markturbo-welcome-paste";
const WELCOME_OPEN_FILE_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-file";
const WELCOME_OPEN_FOLDER_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-folder";
const WELCOME_OPEN_SAMPLE_ACCESSIBILITY_ID: &str = "markturbo-welcome-open-sample";
const WELCOME_DONT_SHOW_ACCESSIBILITY_ID: &str = "markturbo-welcome-dont-show-again";
const WELCOME_KEY_CONTEXT: &str = "Welcome";

fn should_show_welcome(initial: Option<&Path>, show_welcome_on_startup: bool) -> bool {
    initial.is_none() && show_welcome_on_startup
}

fn recent_target_issue(target: &crate::settings::RecentTarget) -> Option<i18n::Key> {
    if !target.path.exists() {
        return Some(i18n::Key::RecentMissing);
    }
    match target.kind {
        crate::settings::RecentTargetKind::File
            if target.path.is_file() && crate::workspace::is_openable(&target.path) =>
        {
            None
        }
        crate::settings::RecentTargetKind::Workspace if target.path.is_dir() => None,
        _ => Some(i18n::Key::RecentUnavailable),
    }
}

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

    for root in mt_doc::skill::discovery_roots() {
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

    let is_instruction = mt_doc::instruction::is_instruction(Path::new(&relative_text));
    let affects_instruction_root = |within: &str| {
        let components: Vec<&str> = within.split('/').collect();
        let first = components.first().copied().unwrap_or_default();
        let nested = matches!(first, "rules" | "instructions" | "memories");

        (components.len() == 1 && (nested || is_instruction))
            || (nested && components.len() <= 3 && is_instruction)
            || (nested && components.len() == 2 && is_directory)
    };
    for root in mt_doc::instruction::project_roots()
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
    welcome_scroll: ScrollHandle,
    /// The deliberate first-run surface, available only for a no-argument start.
    show_welcome: bool,
    /// Filesystem availability captured when the Welcome surface is entered.
    ///
    /// Rendering may occur many times while a window is resized or animated.
    /// Recent-target and sample probes belong at that state boundary instead of
    /// synchronously touching the filesystem from `render_welcome`.
    welcome_recent_issues: HashMap<PathBuf, Option<i18n::Key>>,
    welcome_sample_available: bool,
    root: Option<PathBuf>,
    explorer: Option<Entity<Explorer>>,
    harness: Option<Entity<HarnessView>>,
    /// Global skill roots outlive the folder-specific Harness view.
    skill_cache: Arc<Mutex<mt_doc::skill::DiscoveryCache>>,
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
    recovery: Option<RecoveryStore>,
    /// True until the startup recovery scan either completes or fails.
    startup_recovery_pending: bool,
    /// Original keys for documents opened before startup recovery is ready.
    /// Save As observes the new path even though an older checkpoint still
    /// belongs to the path that was open when startup began.
    startup_recovery_keys: HashMap<crate::lifecycle::DocumentId, RecoveryKey>,
    /// The dirty source key that a successful Save As must retire. The document
    /// has its new file identity by the time it emits `DirtyChanged`.
    save_as_recovery_keys: HashMap<crate::lifecycle::DocumentId, RecoveryKey>,
    /// Explicit Save or Discard decisions that still need a durable marker.
    /// `None` keeps unknown-origin work fail-closed for every destructive action.
    pending_recovery_retirements: HashMap<RecoveryKey, Option<DocumentId>>,
    recovery_retirements: HashMap<RecoveryKey, RecoveryRetirement>,
    recovery_retirement_batches: HashMap<RecoveryKey, RecoveryRetirementBatch>,
    recovery_retirement_retries: HashSet<RecoveryKey>,
    recovery_schedules: HashMap<crate::lifecycle::DocumentId, DocumentRecoveryState>,
    /// True while the one physical checkpoint batch owned by this workspace is running.
    recovery_checkpoint_worker_active: bool,
    recovery_warning: Option<String>,
    status: Option<String>,
    /// Bumped by every [`Workspace::set_status`], so a timer can tell whether
    /// the message it was started for is still the one on screen.
    status_generation: u64,
    /// The timer that clears the current status message.
    ///
    /// One slot rather than a detached task per message: replacing it cancels
    /// the previous timer, which is the other half of the generation check.
    _status_timer: Option<Task<()>>,
    /// One wake-up for the earliest dirty-buffer checkpoint deadline.
    _recovery_timer: Option<Task<()>>,
    /// Bumped whenever the single recovery wake-up is replaced or cancelled.
    /// A task can wake while it is being dropped, so its generation is also
    /// checked before it is allowed to dispatch background checkpoint work.
    recovery_timer_generation: u64,
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
    /// Present while Save / Discard / Cancel is resolving a destructive action.
    pending_destructive: Option<DestructiveRequest>,
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

struct DocumentRecoveryState {
    key: RecoveryKey,
    revision: u64,
    suppressed_oversized_revision: Option<u64>,
    token: Option<RecoveryToken>,
    schedule: CheckpointSchedule,
    in_flight: Option<RecoveryAttempt>,
    /// The current due boundary has already cancelled or warned while the
    /// physical workspace worker remains occupied.
    deadline_reported: bool,
    protection_warning: bool,
}

#[derive(Debug, Clone)]
struct RecoveryAttempt {
    token: RecoveryToken,
    revision: u64,
    timing: CheckpointAttemptTiming,
    cancelled: Arc<AtomicBool>,
}

impl RecoveryAttempt {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl PartialEq for RecoveryAttempt {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token
            && self.revision == other.revision
            && self.timing == other.timing
            && Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

impl Eq for RecoveryAttempt {}

fn current_checkpoint_write_completed(
    attempt_is_current: bool,
    state_revision: u64,
    attempt_revision: u64,
    outcome: &CheckpointBatchOutcome,
) -> bool {
    attempt_is_current
        && state_revision == attempt_revision
        && matches!(outcome, CheckpointBatchOutcome::Written)
}

#[derive(Default)]
struct StartupRecovery {
    recovery: Option<RecoveryStore>,
    documents: Vec<PreparedRecovery>,
    recovery_issue_count: usize,
    recovery_error: Option<String>,
}

struct PendingStartupDestructive {
    request: DestructiveRequest,
    keys: Vec<(RecoveryKey, Option<DocumentId>)>,
}

fn insert_scoped_recovery_key(
    keys: &mut HashMap<RecoveryKey, Option<DocumentId>>,
    key: RecoveryKey,
    document_id: Option<DocumentId>,
) -> Option<DocumentId> {
    *keys
        .entry(key)
        .and_modify(|current| {
            if *current != document_id {
                *current = None;
            }
        })
        .or_insert(document_id)
}

fn prepare_recovery_records(records: Vec<RecoveredRecord>) -> (Vec<PreparedRecovery>, usize) {
    let mut documents = Vec::with_capacity(records.len());
    let mut skipped = 0;
    for recovered in records {
        match DocumentView::prepare_recovery(recovered) {
            Ok(document) => documents.push(document),
            Err(_) => skipped += 1,
        }
    }
    (documents, skipped)
}

/// Open, verify, and parse recovery data without making it a prerequisite for editing.
fn startup_recovery() -> StartupRecovery {
    #[cfg(not(test))]
    {
        match RecoveryStore::open() {
            Ok((store, maintenance)) => match store.recover() {
                Ok(scan) => {
                    let scan_issues = scan.issues.len();
                    let (documents, preparation_issues) = prepare_recovery_records(scan.records);
                    StartupRecovery {
                        recovery: Some(store),
                        documents,
                        recovery_issue_count: maintenance.issues.len()
                            + scan_issues
                            + preparation_issues,
                        recovery_error: None,
                    }
                }
                Err(error) => StartupRecovery {
                    recovery: Some(store),
                    recovery_issue_count: maintenance.issues.len(),
                    recovery_error: Some(error.to_string()),
                    ..StartupRecovery::default()
                },
            },
            Err(error) => StartupRecovery {
                recovery_error: Some(error.to_string()),
                ..StartupRecovery::default()
            },
        }
    }
    #[cfg(test)]
    {
        // Tests install an explicit reversible protector when they need durable
        // records; opening production DPAPI storage would make test state leak
        // across runs and hide which records a test owns.
        StartupRecovery::default()
    }
}

fn startup_recovery_status(
    restored: usize,
    skipped: usize,
    recovery_error: Option<&str>,
) -> Option<String> {
    let summary = (restored > 0 || skipped > 0).then(|| {
        format!(
            "Restored {restored} recovery checkpoint(s); skipped {skipped} unavailable or invalid record(s)."
        )
    });
    match (recovery_error, summary) {
        (Some(error), Some(summary)) => {
            Some(format!("{error}. Editing remains available. {summary}"))
        }
        (Some(error), None) => Some(format!("{error}. Editing remains available.")),
        (None, Some(summary)) => Some(summary),
        (None, None) => None,
    }
}

fn checkpoint_batch_status(maintenance_issues: usize, last_error: Option<&str>) -> Option<String> {
    let maintenance = (maintenance_issues > 0).then(|| {
        format!(
            "Recovery skipped {maintenance_issues} malformed, oversized, expired, or unreadable record(s)."
        )
    });
    match (last_error, maintenance) {
        (Some(error), Some(maintenance)) => Some(format!(
            "{error}. Editing and source files are unchanged. {maintenance}"
        )),
        (Some(error), None) => Some(format!("{error}. Editing and source files are unchanged.")),
        (None, Some(maintenance)) => Some(maintenance),
        (None, None) => None,
    }
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
}

impl Workspace {
    /// Create the workspace, opening `initial` if given.
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_startup_recovery(initial, startup_recovery, window, cx)
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
            welcome_scroll: ScrollHandle::new(),
            show_welcome: should_show_welcome(
                initial.as_deref(),
                crate::settings::AppSettings::global(cx).show_welcome_on_startup,
            ),
            welcome_recent_issues: HashMap::new(),
            welcome_sample_available: false,
            root: None,
            explorer: None,
            harness: None,
            skill_cache: Arc::new(Mutex::new(mt_doc::skill::DiscoveryCache::default())),
            search: cx.new(|cx| SearchView::new(window, cx)),
            settings: cx.new(SettingsView::new),
            side_panel: SidePanel::Files,
            tabs: Tabs::default(),
            history: History::default(),
            registry: Arc::new(RendererRegistry::with_defaults()),
            watcher: None,
            recovery: None,
            startup_recovery_pending: true,
            startup_recovery_keys: HashMap::new(),
            save_as_recovery_keys: HashMap::new(),
            pending_recovery_retirements: HashMap::new(),
            recovery_retirements: HashMap::new(),
            recovery_retirement_batches: HashMap::new(),
            recovery_retirement_retries: HashSet::new(),
            recovery_schedules: HashMap::new(),
            recovery_checkpoint_worker_active: false,
            recovery_warning: None,
            status: None,
            status_generation: 0,
            _status_timer: None,
            _recovery_timer: None,
            recovery_timer_generation: 0,
            settings_open: false,
            preferred_left_panel_width: metrics::SIDE_PANEL.resolve(viewport),
            preferred_right_panel_width: metrics::RIGHT_PANEL.resolve(viewport),
            layout_width: None,
            panel_resize_grab: None,
            left_panel_open: true,
            right_panel_open: true,
            translating: false,
            pending_destructive: None,
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
                SearchEvent::Reveal { path, offset } => {
                    this.reveal_in(path.clone(), *offset, window, cx);
                }
            },
        ));
        // The page writes the setting; what it cannot do is repaint the rest of
        // the app. Each event names what changed, and the response is chosen
        // here — the WebView caches HTML with the palette baked in, which is not
        // something a settings page should have to know.
        let settings = this.settings.clone();
        this._subscriptions.push(cx.subscribe(
            &settings,
            |this: &mut Self, _, event: &SettingsEvent, cx| match event {
                SettingsEvent::ThemeChanged => this.reapply_theme(cx),
                SettingsEvent::LanguageChanged => this.relabel(cx),
                SettingsEvent::SkillScopeChanged => this.rescan_harness(cx),
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
                let _ = &this;
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
                    != crate::settings::ThemePreference::System
                {
                    return;
                }
                crate::settings::apply_theme(
                    crate::settings::ThemePreference::System,
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
        let startup_targets = this.startup_recovery_targets(cx);
        cx.defer_in(window, move |this, window, cx| {
            this.start_startup_recovery(load_startup_recovery, startup_targets, window, cx);
        });
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
        let harness = cx.new(|cx| HarnessView::new(path.clone(), skill_cache, cx));

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
                |this: &mut Self, _, event: &HarnessEvent, window, cx| {
                    let HarnessEvent::OpenFile { path, preview } = event;
                    this.open_file_as(path.clone(), *preview, window, cx);
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
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
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

        // Replace the outgoing preview rather than accumulating tabs. Its edits
        // are the one thing that must not be discarded silently, so a dirty
        // preview is kept and simply stops being one.
        if preview
            && let Some(current) = self.tabs.take_preview()
            && let Some(ix) = self.tabs.index_of(&current)
        {
            let preserve = self.tabs.get(ix).is_some_and(|tab| {
                let document = tab.payload.view.read(cx);
                let key = self
                    .startup_recovery_keys
                    .get(&document.id())
                    .cloned()
                    .unwrap_or_else(|| document.recovery_key());
                document.is_dirty() || self.is_undurable_recovery_retirement(&key)
            });
            if preserve {
                // Keep it: promoting beats losing unsaved work.
            } else {
                self.close_tab_unchecked(ix, cx);
            }
        }

        let opened = self.open_file_inner(path.clone(), window, cx);
        self.tabs.set_preview((preview && opened).then_some(path));
        cx.notify();
        opened
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
        if !path.is_file() || !crate::workspace::is_openable(&path) {
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

    fn record_recent_file(&self, path: PathBuf, cx: &mut Context<Self>) {
        self.record_recent_target(path, crate::settings::RecentTargetKind::File, cx);
    }

    fn record_recent_workspace(&self, path: PathBuf, cx: &mut Context<Self>) {
        self.record_recent_target(path, crate::settings::RecentTargetKind::Workspace, cx);
    }

    fn record_recent_target(
        &self,
        path: PathBuf,
        kind: crate::settings::RecentTargetKind,
        cx: &mut Context<Self>,
    ) {
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let target = crate::settings::RecentTarget::new(path, kind, display_name);
        if crate::settings::AppSettings::global(cx)
            .recent_targets
            .first()
            == Some(&target)
        {
            return;
        }
        crate::settings::AppSettings::update(cx, move |settings| {
            settings.record_recent_target(target);
        });
    }

    fn open_recent_target(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = crate::settings::AppSettings::global(cx)
            .recent_targets
            .iter()
            .find(|target| target.path == path)
            .cloned();
        let Some(target) = target else { return false };
        if recent_target_issue(&target).is_some() {
            return false;
        }
        self.open_target(target.path, true, window, cx)
    }

    /// Refresh Welcome-only filesystem state once when the surface is entered.
    ///
    /// Opening an item always repeats this check, because the cache is only a
    /// presentation hint and must never authorize an operation on stale data.
    fn refresh_welcome_availability(&mut self, cx: &App) {
        self.welcome_recent_issues = crate::settings::AppSettings::global(cx)
            .recent_targets
            .iter()
            .map(|target| (target.path.clone(), recent_target_issue(target)))
            .collect();
        self.welcome_sample_available = crate::app_paths::bundled_sample_dir().is_some();
    }

    fn welcome_recent_target_issue(
        &self,
        target: &crate::settings::RecentTarget,
    ) -> Option<i18n::Key> {
        self.welcome_recent_issues
            .get(&target.path)
            .copied()
            .flatten()
    }

    fn remove_recent_target(&mut self, path: &Path, cx: &mut Context<Self>) {
        crate::settings::AppSettings::update(cx, |settings| {
            settings.remove_recent_target(path);
        });
    }

    fn open_bundled_sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = crate::app_paths::bundled_sample_dir() {
            self.open_target(path, true, window, cx);
        }
    }

    fn dont_show_welcome_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::AppSettings::update(cx, |settings| {
            settings.show_welcome_on_startup = false;
        });
        self.new_memory(String::new(), window, cx);
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
        if self.startup_recovery_pending {
            let document = view.read(cx);
            self.startup_recovery_keys
                .entry(document.id())
                .or_insert_with(|| document.recovery_key());
        }
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
                        let key = document.read(cx).recovery_key();
                        this.pending_recovery_retirements.remove(&key);
                        this.arm_document_recovery(document, cx);
                    }
                    DocumentEvent::DirtyChanged => {
                        if !document.read(cx).is_dirty() {
                            let id = document.read(cx).id();
                            let current_key = document.read(cx).recovery_key();
                            let key = this
                                .startup_recovery_keys
                                .remove(&id)
                                .or_else(|| this.save_as_recovery_keys.remove(&id))
                                .unwrap_or(current_key);
                            this.retire_document_recovery(id, Some(key), cx);
                        }
                        cx.notify();
                    }
                    DocumentEvent::ScrollWebPreview(fraction) => {
                        this.queue_web_scroll(*fraction, cx)
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

    fn restore_prepared_recovery(
        &mut self,
        documents: Vec<PreparedRecovery>,
        startup_targets: Option<&HashMap<PathBuf, (crate::lifecycle::DocumentId, u64)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, usize) {
        let mut restored = 0;
        let mut skipped = 0;
        for prepared in documents {
            let source_path = prepared.source_path().map(Path::to_path_buf);
            if let Some(ref path) = source_path
                && let Some(ix) = self.tabs.index_of(path)
            {
                let Some(startup_targets) = startup_targets else {
                    skipped += 1;
                    continue;
                };
                let expected = startup_targets.get(path).copied();

                let document = self
                    .document_at(ix)
                    .cloned()
                    .expect("an indexed recovery path must have a document");
                let applied = document.update(cx, |document, cx| {
                    if !document.can_accept_startup_recovery(expected) {
                        return false;
                    }
                    document.apply_startup_recovery(prepared, window, cx);
                    true
                });
                if applied {
                    self.register_restored_recovery(&document, cx);
                    restored += 1;
                } else {
                    skipped += 1;
                }
                continue;
            }

            let registry = self.registry.clone();
            let view = cx.new(|cx| DocumentView::from_recovery(prepared, registry, window, cx));
            if let Some(path) = source_path {
                self.insert_document_with_recovery(
                    TabIdentity::File(path),
                    view.clone(),
                    false,
                    window,
                    cx,
                );
            } else {
                self.insert_memory_document(view.clone(), false, window, cx);
            }
            self.register_restored_recovery(&view, cx);
            restored += 1;
        }
        (restored, skipped)
    }

    fn register_restored_recovery(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery.clone() else {
            return;
        };
        let (id, key, revision) = {
            let document = document.read(cx);
            (document.id(), document.recovery_key(), document.revision())
        };
        let (token, protection_warning) = store.activate_and_current_token(&key);
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_durable_baseline(cx.background_executor().now());
        self.recovery_schedules.insert(
            id,
            DocumentRecoveryState {
                key,
                revision,
                suppressed_oversized_revision: None,
                token: Some(token),
                schedule,
                in_flight: None,
                deadline_reported: false,
                protection_warning,
            },
        );
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    fn start_startup_recovery<F>(
        &mut self,
        load_startup_recovery: F,
        startup_targets: HashMap<PathBuf, (crate::lifecycle::DocumentId, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce() -> StartupRecovery + Send + 'static,
    {
        let task = cx.spawn_in(window, async move |this, cx| {
            let startup = cx
                .background_spawn(async move { load_startup_recovery() })
                .await;
            let startup = Arc::new(Mutex::new(Some((startup, startup_targets))));
            loop {
                let startup = startup.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let startup = startup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((startup, startup_targets)) = startup {
                        this.restore_startup_recovery(startup, startup_targets, window, cx);
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
        });
        self._tasks.push(task);
    }

    fn startup_recovery_targets(
        &self,
        cx: &App,
    ) -> HashMap<PathBuf, (crate::lifecycle::DocumentId, u64)> {
        self.tabs
            .iter()
            .filter_map(|tab| {
                let document = tab.payload.view.read(cx);
                tab.path()
                    .map(|path| (path.to_path_buf(), (document.id(), document.revision())))
            })
            .collect()
    }

    fn resume_recovery_destructive(
        &mut self,
        mut pending: PendingStartupDestructive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match pending.request.revalidate(&self.lifecycle_documents(cx)) {
            DestructiveResolution::Prompt(_) => {
                self.rearm_dirty_recovery(cx);
                self.pending_destructive_recovery.extend(pending.keys);
                self.pending_destructive = Some(pending.request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(action) => self.perform_after_discard_retirement(
                pending.request,
                action,
                pending.keys,
                window,
                cx,
            ),
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.rearm_dirty_recovery(cx);
            }
        }
    }

    fn restore_startup_recovery(
        &mut self,
        mut startup: StartupRecovery,
        startup_targets: HashMap<PathBuf, (crate::lifecycle::DocumentId, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        startup.documents.retain(|document| {
            !self
                .pending_recovery_retirements
                .contains_key(&document.recovery_key())
        });
        self.recovery = startup.recovery.take();
        self.startup_recovery_pending = false;
        self.startup_recovery_keys.clear();
        let pending_startup_destructive = self.pending_startup_destructive.take();
        let (restored, restore_skipped) =
            self.restore_prepared_recovery(startup.documents, Some(&startup_targets), window, cx);
        if let Some(status) = startup_recovery_status(
            restored,
            startup.recovery_issue_count + restore_skipped,
            startup.recovery_error.as_deref(),
        ) {
            self.set_status(status, cx);
        }
        debug_assert!(!self.startup_recovery_pending);
        log::debug!("recovery startup finished");
        if self.recovery.is_none() {
            if pending_startup_destructive.is_some()
                || !self.pending_recovery_retirements.is_empty()
            {
                self.set_status(
                    "Recovery storage is unavailable, so its checkpoint could not be cleared. The document remains open."
                        .into(),
                    cx,
                );
            }
            return;
        }

        if let Some(pending) = pending_startup_destructive {
            self.resume_recovery_destructive(pending, window, cx);
        } else {
            self.flush_pending_recovery_retirements(cx);
        }
        let dirty_documents: Vec<_> = self
            .document_views()
            .into_iter()
            .filter(|document| {
                let document = document.read(cx);
                let key = document.recovery_key();
                document.is_dirty()
                    && !self.pending_recovery_retirements.contains_key(&key)
                    && !self.recovery_retirements.contains_key(&key)
                    && !self.recovery_retirement_batches.contains_key(&key)
                    && self
                        .recovery_schedules
                        .get(&document.id())
                        .is_none_or(|state| state.token.is_none())
            })
            .collect();
        for document in dirty_documents {
            self.arm_document_recovery(&document, cx);
        }
    }

    fn flush_pending_recovery_retirements(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<_> = self
            .pending_recovery_retirements
            .iter()
            .map(|(key, document_id)| (key.clone(), *document_id))
            .collect();
        for (key, document_id) in pending {
            self.invalidate_recovery(&key, document_id, cx);
        }
    }

    fn pending_recovery_keys(
        &self,
        action: &DestructiveAction,
    ) -> Vec<(RecoveryKey, Option<DocumentId>)> {
        self.pending_recovery_retirements
            .iter()
            .filter_map(|(key, document_id)| match action {
                DestructiveAction::CloseTab(id) => (document_id.is_none()
                    || *document_id == Some(*id))
                .then(|| (key.clone(), *document_id)),
                DestructiveAction::CloseWindow | DestructiveAction::ReplaceWorkspace(_) => {
                    Some((key.clone(), *document_id))
                }
            })
            .collect()
    }

    fn is_undurable_recovery_retirement(&self, key: &RecoveryKey) -> bool {
        self.pending_recovery_retirements.contains_key(key)
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

    fn document_index(&self, id: crate::lifecycle::DocumentId, cx: &App) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.payload.view.read(cx).id() == id)
    }

    fn document_by_id(
        &self,
        id: crate::lifecycle::DocumentId,
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
            self.prompt_save_as(document.read(cx).id(), window, cx);
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

    fn save_as_snapshot_is_current(&self, id: crate::lifecycle::DocumentId, cx: &App) -> bool {
        match self.pending_destructive.as_ref() {
            None => true,
            Some(request) => {
                request.current() == Some(id)
                    && request.current_prompt_matches(&self.lifecycle_documents(cx))
            }
        }
    }

    fn cancel_pending_destructive_save_as(&mut self, id: crate::lifecycle::DocumentId) {
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
        id: crate::lifecycle::DocumentId,
        saved_snapshot: crate::lifecycle::BufferSnapshot,
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

    fn perform_after_discard_retirement(
        &mut self,
        request: DestructiveRequest,
        action: DestructiveAction,
        mut scoped_keys: Vec<(RecoveryKey, Option<DocumentId>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        scoped_keys.extend(self.pending_recovery_keys(&action));
        let mut merged = HashMap::new();
        for (key, document_id) in scoped_keys {
            insert_scoped_recovery_key(&mut merged, key, document_id);
        }
        let scoped_keys: Vec<_> = merged.into_iter().collect();
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys.iter().any(|key| {
            self.pending_recovery_retirements.contains_key(key)
                && (self.recovery_retirements.contains_key(key)
                    || self.recovery_retirement_batches.contains_key(key))
        }) {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        let dirty_keys: HashSet<_> = self
            .document_views()
            .into_iter()
            .filter_map(|document| {
                let document = document.read(cx);
                document.is_dirty().then(|| document.recovery_key())
            })
            .collect();
        if keys.iter().any(|key| {
            dirty_keys.contains(key)
                && (self.recovery_retirements.contains_key(key)
                    || self.recovery_retirement_batches.contains_key(key))
        }) {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        let scoped_keys: Vec<_> = scoped_keys
            .into_iter()
            .filter(|(key, _)| {
                !self.recovery_retirements.contains_key(key)
                    && !self.recovery_retirement_batches.contains_key(key)
            })
            .collect();
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys
            .iter()
            .any(|key| self.recovery_retirement_retries.contains(key))
        {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        if keys.is_empty() {
            self.perform_destructive(action, window, cx);
            return;
        }
        let Some(store) = self.recovery.clone() else {
            for (key, document_id) in &scoped_keys {
                insert_scoped_recovery_key(
                    &mut self.pending_recovery_retirements,
                    key.clone(),
                    *document_id,
                );
                self.remove_recovery_state_for_key(key, cx);
            }
            if self.startup_recovery_pending {
                self.pending_startup_destructive = Some(PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                });
                self.set_status(
                    "Waiting for recovery storage to clear its checkpoint. The document remains open."
                        .into(),
                    cx,
                );
            } else {
                self.set_status(
                    "Recovery storage is unavailable, so its checkpoint could not be cleared. The document remains open."
                        .into(),
                    cx,
                );
            }
            return;
        };

        for (key, document_id) in &scoped_keys {
            insert_scoped_recovery_key(
                &mut self.pending_recovery_retirements,
                key.clone(),
                *document_id,
            );
        }
        let now = cx.background_executor().now();
        for key in &keys {
            self.cancel_recovery_attempts_for_key(key, now);
        }
        let batch = match store.begin_retirements(keys.iter().cloned()) {
            Ok(batch) => batch,
            Err(error) => {
                self.rearm_dirty_recovery(cx);
                self.set_status(
                    format!(
                        "Could not clear the recovery checkpoint: {error}. The document remains open."
                    ),
                    cx,
                );
                return;
            }
        };
        for key in &keys {
            self.pending_recovery_retirements.remove(key);
            self.recovery_retirement_batches
                .insert(key.clone(), batch.clone());
        }
        self.pending_destructive = Some(request);
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);

        cx.spawn_in(window, async move |this, cx| {
            let completed_batch = batch.clone();
            let completed = cx
                .background_spawn(async move {
                    let result = store.complete_retirements(batch.clone());
                    if result.is_err() {
                        store.abandon_retirements(&batch);
                    }
                    result
                })
                .await;
            let completed = Arc::new(Mutex::new(Some((scoped_keys, completed_batch, completed))));
            loop {
                let completed_for_update = completed.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let completed = completed_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(completed) = completed {
                        let (scoped_keys, batch, result) = completed;
                        this.finish_discard_retirements(scoped_keys, batch, result, window, cx);
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

    fn schedule_destructive_retirement_continuation(
        &mut self,
        pending: PendingStartupDestructive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_startup_destructive = Some(pending);
        self.set_status(
            "Waiting for recovery checkpoint cleanup before continuing. The document remains open."
                .into(),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            loop {
                if crate::views::try_update_in(&this, cx, |this, window, cx| {
                    if let Some(pending) = this.pending_startup_destructive.take() {
                        this.resume_recovery_destructive(pending, window, cx);
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

    fn finish_discard_retirements(
        &mut self,
        scoped_keys: Vec<(RecoveryKey, Option<DocumentId>)>,
        batch: RecoveryRetirementBatch,
        result: Result<RetirementCompletion, RecoveryError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys
            .iter()
            .any(|key| self.recovery_retirement_batches.get(key) != Some(&batch))
        {
            if let Some(request) = self.pending_destructive.take() {
                self.resume_recovery_destructive(
                    PendingStartupDestructive {
                        request,
                        keys: scoped_keys,
                    },
                    window,
                    cx,
                );
            }
            return;
        }
        let Some(mut request) = self.pending_destructive.take() else {
            return;
        };

        let mut wait_for_replayed_retirement = false;
        let cleanup_error = match result {
            Ok(RetirementCompletion::Retired { .. }) => {
                self.finish_recovery_retirement_batch(&keys, &batch, cx);
                wait_for_replayed_retirement = keys
                    .iter()
                    .any(|key| self.pending_recovery_retirements.contains_key(key));
                None
            }
            Ok(RetirementCompletion::CleanupPending { error }) => {
                self.schedule_recovery_retirement_batch_retry(keys.clone(), batch.clone(), cx);
                Some(format!(
                    "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                ))
            }
            Err(error) => {
                self.finish_recovery_retirement_batch(&keys, &batch, cx);
                self.rearm_dirty_recovery(cx);
                self.set_status(
                    format!(
                        "Could not clear the recovery checkpoint: {error}. The document remains open."
                    ),
                    cx,
                );
                return;
            }
        };
        if wait_for_replayed_retirement {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }

        match request.revalidate(&self.lifecycle_documents(cx)) {
            DestructiveResolution::Prompt(_) => {
                self.rearm_dirty_recovery(cx);
                self.pending_destructive_recovery.extend(scoped_keys);
                self.pending_destructive = Some(request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(action) => {
                for (key, _) in scoped_keys {
                    self.remove_recovery_state_for_key(&key, cx);
                }
                if let Some(error) = cleanup_error {
                    self.set_status(error, cx);
                }
                self.perform_destructive(action, window, cx);
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.rearm_dirty_recovery(cx);
            }
        }
    }

    fn rearm_dirty_recovery(&mut self, cx: &mut Context<Self>) {
        let dirty: Vec<_> = self
            .document_views()
            .into_iter()
            .filter(|document| document.read(cx).is_dirty())
            .collect();
        for document in dirty {
            self.arm_document_recovery(&document, cx);
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
            // A close is the last intentional lifecycle decision for this
            // buffer. Cancel its deadline before invalidating any checkpoint
            // capability held by an already-running worker.
            self.retire_document_recovery(id, None, cx);
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

    fn refresh_recovery_warning(&mut self, cx: &mut Context<Self>) {
        let warning = self
            .recovery_schedules
            .values()
            .any(|state| state.protection_warning)
            .then(|| {
                "Recovery protection is unavailable for at least one dirty document. Editing and source files are unchanged."
                    .to_string()
            });
        if self.recovery_warning != warning {
            self.recovery_warning = warning;
            cx.notify();
        }
    }

    /// Remove one document's deadline before invalidating checkpoint work.
    fn remove_recovery_schedule(
        &mut self,
        id: crate::lifecycle::DocumentId,
        cx: &mut Context<Self>,
    ) -> Option<RecoveryKey> {
        let key = self.recovery_schedules.remove(&id).map(|state| {
            if let Some(attempt) = &state.in_flight {
                attempt.cancel();
            }
            state.key
        });
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
        key
    }

    /// Invalidate in-flight checkpoint capabilities before deleting durable data.
    fn invalidate_recovery(
        &mut self,
        key: &RecoveryKey,
        document_id: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        let document_id = insert_scoped_recovery_key(
            &mut self.pending_recovery_retirements,
            key.clone(),
            document_id,
        );
        let had_owner = self.recovery_retirements.contains_key(key)
            || self.recovery_retirement_batches.contains_key(key);
        let Some(store) = self.recovery.clone() else {
            return;
        };
        let ticket = match store.begin_retirement(key) {
            Ok(ticket) => ticket,
            Err(error) => {
                self.set_status(
                    format!("Could not clear the recovery checkpoint: {error}"),
                    cx,
                );
                if !had_owner {
                    self.schedule_recovery_retirement_retry(key.clone(), cx);
                }
                return;
            }
        };
        self.pending_recovery_retirements.remove(key);
        let stale_batch = self.recovery_retirement_batches.get(key).cloned();
        let stale_batch_keys: Vec<_> = stale_batch
            .as_ref()
            .map(|batch| {
                self.recovery_retirement_batches
                    .iter()
                    .filter(|(_, current)| *current == batch)
                    .map(|(key, _)| key.clone())
                    .collect()
            })
            .unwrap_or_default();
        for stale_key in &stale_batch_keys {
            self.recovery_retirement_batches.remove(stale_key);
            self.recovery_retirement_retries.remove(stale_key);
        }
        self.recovery_retirements
            .insert(key.clone(), ticket.clone());
        self.spawn_recovery_retirement_completion(
            key.clone(),
            ticket,
            document_id,
            Duration::ZERO,
            cx,
        );
        for stale_key in stale_batch_keys {
            if stale_key != *key
                && let Some(&document_id) = self.pending_recovery_retirements.get(&stale_key)
            {
                self.invalidate_recovery(&stale_key, document_id, cx);
            }
        }
    }

    fn spawn_recovery_retirement_completion(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let completed_ticket = ticket.clone();
            let result = cx
                .background_spawn(async move {
                    let result = store.complete_retirement(ticket.clone());
                    if result.is_err() {
                        store.abandon_retirement(&ticket);
                    }
                    result
                })
                .await;
            let result = Arc::new(Mutex::new(Some(result)));
            loop {
                let key = key.clone();
                let ticket = completed_ticket.clone();
                let result_for_update = result.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    if this.recovery_retirements.get(&key) != Some(&ticket) {
                        return;
                    }
                    let result = result_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(result) = result {
                        this.finish_recovery_retirement(key, ticket, document_id, result, cx);
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

    fn finish_recovery_retirement(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        result: Result<RetirementCompletion, RecoveryError>,
        cx: &mut Context<Self>,
    ) {
        if self.recovery_retirements.get(&key) != Some(&ticket) {
            return;
        }
        self.recovery_retirement_retries.remove(&key);
        match result {
            Ok(RetirementCompletion::Retired { .. }) => {
                self.recovery_retirements.remove(&key);
                if let Some(&document_id) = self.pending_recovery_retirements.get(&key) {
                    self.invalidate_recovery(&key, document_id, cx);
                }
            }
            Ok(RetirementCompletion::CleanupPending { error }) => {
                self.set_status(
                    format!(
                        "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                    ),
                    cx,
                );
                self.schedule_recovery_retirement_completion_retry(key, ticket, document_id, cx);
            }
            Err(error) => {
                self.recovery_retirements.remove(&key);
                insert_scoped_recovery_key(
                    &mut self.pending_recovery_retirements,
                    key.clone(),
                    document_id,
                );
                self.set_status(
                    format!("Could not clear the recovery checkpoint: {error}"),
                    cx,
                );
                self.schedule_recovery_retirement_retry(key, cx);
            }
        }
    }

    fn schedule_recovery_retirement_completion_retry(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        if !self.recovery_retirement_retries.insert(key.clone()) {
            return;
        }
        self.spawn_recovery_retirement_completion(
            key,
            ticket,
            document_id,
            Duration::from_secs(1),
            cx,
        );
    }

    fn finish_recovery_retirement_batch(
        &mut self,
        keys: &[RecoveryKey],
        batch: &RecoveryRetirementBatch,
        cx: &mut Context<Self>,
    ) {
        if keys
            .iter()
            .any(|key| self.recovery_retirement_batches.get(key) != Some(batch))
        {
            return;
        }
        for key in keys {
            self.recovery_retirement_batches.remove(key);
            self.recovery_retirement_retries.remove(key);
        }
        let queued: Vec<_> = keys
            .iter()
            .filter_map(|key| {
                self.pending_recovery_retirements
                    .get(key)
                    .map(|document_id| (key.clone(), *document_id))
            })
            .collect();
        for (key, document_id) in queued {
            self.invalidate_recovery(&key, document_id, cx);
        }
    }

    fn schedule_recovery_retirement_batch_retry(
        &mut self,
        keys: Vec<RecoveryKey>,
        batch: RecoveryRetirementBatch,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty()
            || keys
                .iter()
                .any(|key| self.recovery_retirement_retries.contains(key))
        {
            return;
        }
        let Some(store) = self.recovery.clone() else {
            return;
        };
        self.recovery_retirement_retries
            .extend(keys.iter().cloned());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(1))
                .await;
            let completed_batch = batch.clone();
            let result = cx
                .background_spawn(async move {
                    let result = store.complete_retirements(batch.clone());
                    if result.is_err() {
                        store.abandon_retirements(&batch);
                    }
                    result
                })
                .await;
            let completed = Arc::new(Mutex::new(Some((keys, completed_batch, result))));
            loop {
                let completed_for_update = completed.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let completed = completed_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((keys, batch, result)) = completed {
                        if keys.iter().any(|key| {
                            this.recovery_retirement_batches.get(key) != Some(&batch)
                        }) {
                            return;
                        }
                        for key in &keys {
                            this.recovery_retirement_retries.remove(key);
                        }
                        match result {
                            Ok(RetirementCompletion::Retired { .. }) => {
                                this.finish_recovery_retirement_batch(&keys, &batch, cx);
                            }
                            Ok(RetirementCompletion::CleanupPending { error }) => {
                                this.set_status(
                                    format!(
                                        "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                                    ),
                                    cx,
                                );
                                this.schedule_recovery_retirement_batch_retry(keys, batch, cx);
                            }
                            Err(error) => {
                                this.finish_recovery_retirement_batch(&keys, &batch, cx);
                                this.rearm_dirty_recovery(cx);
                                this.set_status(
                                    format!(
                                        "Could not finish recovery checkpoint cleanup: {error}"
                                    ),
                                    cx,
                                );
                            }
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

    fn schedule_recovery_retirement_retry(&mut self, key: RecoveryKey, cx: &mut Context<Self>) {
        if !self.recovery_retirement_retries.insert(key.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            loop {
                let key = key.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let pending = this.pending_recovery_retirements.get(&key).copied();
                    let owned = this.recovery_retirements.contains_key(&key)
                        || this.recovery_retirement_batches.contains_key(&key);
                    if owned {
                        return;
                    }
                    this.recovery_retirement_retries.remove(&key);
                    if let Some(document_id) = pending {
                        this.invalidate_recovery(&key, document_id, cx);
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

    fn retire_document_recovery(
        &mut self,
        id: crate::lifecycle::DocumentId,
        fallback_key: Option<RecoveryKey>,
        cx: &mut Context<Self>,
    ) -> Option<RecoveryKey> {
        let key = self.remove_recovery_schedule(id, cx).or(fallback_key);
        if let Some(key) = &key {
            self.invalidate_recovery(key, Some(id), cx);
        }
        key
    }

    fn cancel_recovery_attempts_for_key(&mut self, key: &RecoveryKey, now: Instant) {
        for state in self
            .recovery_schedules
            .values_mut()
            .filter(|state| state.key == *key)
        {
            if let Some(attempt) = state.in_flight.take() {
                attempt.cancel();
                if now >= attempt.timing.durable_complete_by {
                    state
                        .schedule
                        .checkpoint_deadline_missed(attempt.timing, now);
                    state.protection_warning = true;
                } else {
                    state.schedule.checkpoint_cancelled(attempt.timing, now);
                }
            }
            state.token = None;
        }
    }

    fn remove_recovery_state_for_key(&mut self, key: &RecoveryKey, cx: &mut Context<Self>) {
        self.recovery_schedules.retain(|_, state| {
            if state.key == *key {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                false
            } else {
                true
            }
        });
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    fn arm_document_recovery(&mut self, document: &Entity<DocumentView>, cx: &mut Context<Self>) {
        self.arm_document_recovery_at(document, cx.background_executor().now(), cx);
    }

    fn arm_document_recovery_at(
        &mut self,
        document: &Entity<DocumentView>,
        now: Instant,
        cx: &mut Context<Self>,
    ) {
        let store = self.recovery.clone();
        let (id, revision, key) = {
            let document = document.read(cx);
            (document.id(), document.revision(), document.recovery_key())
        };
        let replaced_key = self.recovery_schedules.get_mut(&id).and_then(|state| {
            (state.key != key).then(|| {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                state.key.clone()
            })
        });
        if let Some(previous_key) = replaced_key {
            self.invalidate_recovery(&previous_key, Some(id), cx);
        }
        match self.recovery_schedules.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut schedule = CheckpointSchedule::default();
                schedule.mark_dirty(now);
                let (token, protection_warning) = store.as_ref().map_or((None, false), |store| {
                    let (token, deferred) = store.activate_and_current_token(&key);
                    (Some(token), deferred)
                });
                entry.insert(DocumentRecoveryState {
                    key,
                    revision,
                    suppressed_oversized_revision: None,
                    token,
                    schedule,
                    in_flight: None,
                    deadline_reported: false,
                    protection_warning,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                if state.key != key {
                    if let Some(attempt) = &state.in_flight {
                        attempt.cancel();
                    }
                    state.key = key.clone();
                    let (token, protection_warning) =
                        store.as_ref().map_or((None, false), |store| {
                            let (token, deferred) = store.activate_and_current_token(&key);
                            (Some(token), deferred)
                        });
                    state.token = token;
                    state.schedule = CheckpointSchedule::default();
                    state.in_flight = None;
                    state.deadline_reported = false;
                    state.suppressed_oversized_revision = None;
                    state.protection_warning = protection_warning;
                } else if state.revision != revision {
                    if let Some(attempt) = &state.in_flight {
                        attempt.cancel();
                    }
                    if state.suppressed_oversized_revision.take().is_some() {
                        state.schedule = CheckpointSchedule::default();
                    }
                }
                if state.token.is_none()
                    && let Some(store) = &store
                {
                    let (token, protection_deferred) = store.activate_and_current_token(&key);
                    state.token = Some(token);
                    state.protection_warning |= protection_deferred;
                }
                state.revision = revision;
                state.schedule.mark_dirty(now);
            }
        }
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    fn schedule_recovery_timer(&mut self, cx: &mut Context<Self>) {
        self.recovery_timer_generation = self.recovery_timer_generation.wrapping_add(1);
        let generation = self.recovery_timer_generation;
        // Dropping the previous task is the primary cancellation mechanism;
        // `generation` also protects the narrow race where it wakes first.
        self._recovery_timer = None;
        let recovery_available = self.recovery.is_some();
        let worker_active = self.recovery_checkpoint_worker_active;
        let now = cx.background_executor().now();
        let Some(deadline) = self
            .recovery_schedules
            .values()
            .filter_map(|state| match &state.in_flight {
                _ if state.suppressed_oversized_revision == Some(state.revision) => None,
                Some(attempt) if !state.deadline_reported => {
                    Some(attempt.timing.durable_complete_by)
                }
                Some(_) => None,
                None if worker_active && state.deadline_reported => None,
                None if recovery_available || !state.protection_warning => {
                    state.schedule.next_deadline()
                }
                None => None,
            })
            .min()
        else {
            return;
        };
        let delay = deadline.saturating_duration_since(now);
        self._recovery_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            loop {
                if crate::views::try_update(&this, cx, |this, cx| {
                    if this.recovery_timer_generation != generation {
                        return;
                    }
                    this._recovery_timer = None;
                    this.checkpoint_recovery(cx);
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
        }));
    }

    fn checkpoint_recovery(&mut self, cx: &mut Context<Self>) {
        self.checkpoint_recovery_at(cx.background_executor().now(), cx);
    }

    fn checkpoint_recovery_at(&mut self, now: Instant, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.clone() else {
            for state in self.recovery_schedules.values_mut() {
                if state.in_flight.is_none() && state.schedule.is_due(now) {
                    state.protection_warning = true;
                }
            }
            self.refresh_recovery_warning(cx);
            self.schedule_recovery_timer(cx);
            return;
        };
        let documents = self.document_views();
        let worker_active = self.recovery_checkpoint_worker_active;
        let mut open_ids = HashSet::new();
        let mut active_keys = HashSet::new();
        let mut due = Vec::new();
        let mut preflight_error = None;

        for document in documents {
            let (id, dirty, revision, key, text_byte_len) = {
                let document = document.read(cx);
                (
                    document.id(),
                    document.is_dirty(),
                    document.revision(),
                    document.recovery_key(),
                    document.text_byte_len(cx),
                )
            };
            open_ids.insert(id);
            if !dirty {
                if let Some(state) = self.recovery_schedules.remove(&id) {
                    self.invalidate_recovery(&state.key, Some(id), cx);
                }
                continue;
            }
            active_keys.insert(key.clone());

            let replaced_key = self.recovery_schedules.get_mut(&id).and_then(|state| {
                (state.key != key).then(|| {
                    if let Some(attempt) = &state.in_flight {
                        attempt.cancel();
                    }
                    state.key.clone()
                })
            });
            if let Some(previous_key) = replaced_key {
                self.invalidate_recovery(&previous_key, Some(id), cx);
            }
            let state = self.recovery_schedules.entry(id).or_insert_with(|| {
                let mut schedule = CheckpointSchedule::default();
                schedule.mark_dirty(now);
                let (token, protection_warning) = store.activate_and_current_token(&key);
                DocumentRecoveryState {
                    key: key.clone(),
                    revision,
                    suppressed_oversized_revision: None,
                    token: Some(token),
                    schedule,
                    in_flight: None,
                    deadline_reported: false,
                    protection_warning,
                }
            });
            if state.key != key {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                state.key = key.clone();
                state.revision = revision;
                let (token, protection_warning) = store.activate_and_current_token(&key);
                state.token = Some(token);
                state.schedule = CheckpointSchedule::default();
                state.schedule.mark_dirty(now);
                state.in_flight = None;
                state.deadline_reported = false;
                state.suppressed_oversized_revision = None;
                state.protection_warning = protection_warning;
            } else if state.revision != revision {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                state.revision = revision;
                if state.suppressed_oversized_revision.take().is_some() {
                    state.schedule = CheckpointSchedule::default();
                }
                state.schedule.mark_dirty(now);
            }
            if let Some(attempt) = state.in_flight.as_ref()
                && now >= attempt.timing.durable_complete_by
                && !state.deadline_reported
            {
                let timing = attempt.timing;
                attempt.cancel();
                state.schedule.checkpoint_deadline_missed(timing, now);
                state.deadline_reported = true;
                state.protection_warning = true;
            }
            if state.in_flight.is_none() && state.schedule.is_due(now) {
                if state.suppressed_oversized_revision == Some(revision) {
                    continue;
                }
                if worker_active {
                    state.deadline_reported = true;
                    state.protection_warning = true;
                    continue;
                }
                let plaintext_ceiling = store.plaintext_admission_ceiling();
                if text_byte_len as u64 > plaintext_ceiling {
                    state.suppressed_oversized_revision = Some(revision);
                    state.protection_warning = true;
                    preflight_error = Some(
                        RecoveryError::OversizedCheckpoint {
                            bytes: text_byte_len as u64,
                            limit: plaintext_ceiling,
                        }
                        .to_string(),
                    );
                    continue;
                }
                // Capture the generation immediately before dispatch. A later
                // Save or Discard invalidates it before deleting the record.
                state.token = Some(store.current_token(&state.key));
                let timing = state
                    .schedule
                    .checkpoint_dispatched(now)
                    .expect("a due dirty recovery schedule must produce attempt timing");
                let attempt = RecoveryAttempt {
                    token: state
                        .token
                        .clone()
                        .expect("a ready recovery store must provide a checkpoint token"),
                    revision,
                    timing,
                    cancelled: Arc::new(AtomicBool::new(false)),
                };
                state.in_flight = Some(attempt.clone());
                state.deadline_reported = false;
                due.push((id, attempt, document.read(cx).recovery_checkpoint(cx)));
            }
        }
        self.recovery_schedules
            .retain(|id, _| open_ids.contains(id));
        self.refresh_recovery_warning(cx);
        if let Some(error) = preflight_error {
            self.set_status(
                checkpoint_batch_status(0, Some(&error))
                    .expect("a checkpoint error must produce visible status"),
                cx,
            );
        }
        if due.is_empty() {
            self.schedule_recovery_timer(cx);
            return;
        }

        debug_assert!(!self.recovery_checkpoint_worker_active);
        self.recovery_checkpoint_worker_active = true;
        self.schedule_recovery_timer(cx);

        cx.spawn(async move |this, cx| {
            let background_executor = cx.background_executor().clone();
            let batch = cx
                .background_spawn(async move {
                    let batch = store.checkpoint_batch_if_current_cancellable(
                        due.iter().map(|(_, attempt, checkpoint)| {
                            CancellableRecoveryCheckpointAttempt {
                                checkpoint,
                                token: &attempt.token,
                                cancelled: attempt.cancelled.as_ref(),
                            }
                        }),
                        &active_keys,
                    );
                    let store_returned_at = background_executor.now();
                    let results = due
                        .into_iter()
                        .zip(batch.outcomes)
                        .map(|((id, attempt, _), outcome)| (id, attempt, outcome))
                        .collect::<Vec<_>>();
                    (results, batch.maintenance, store_returned_at)
                })
                .await;

            let batch = Arc::new(Mutex::new(Some(batch)));
            loop {
                let batch = batch.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let batch = batch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((results, maintenance, store_returned_at)) = batch {
                        this.finish_recovery_checkpoints(
                            results,
                            maintenance,
                            store_returned_at,
                            cx,
                        );
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

    fn finish_recovery_checkpoints(
        &mut self,
        results: Vec<(
            crate::lifecycle::DocumentId,
            RecoveryAttempt,
            CheckpointBatchOutcome,
        )>,
        maintenance: RecoveryMaintenance,
        store_returned_at: Instant,
        cx: &mut Context<Self>,
    ) {
        let now = cx.background_executor().now();
        let worker_released = std::mem::take(&mut self.recovery_checkpoint_worker_active);
        let maintenance_issues = maintenance.issues.len();
        let mut last_error = None;
        for (id, attempt, outcome) in results {
            let attempt_is_current = self
                .recovery_schedules
                .get(&id)
                .is_some_and(|state| state.in_flight.as_ref() == Some(&attempt));
            let written_revision_is_current =
                self.recovery_schedules.get(&id).is_some_and(|state| {
                    current_checkpoint_write_completed(
                        attempt_is_current,
                        state.revision,
                        attempt.revision,
                        &outcome,
                    )
                });
            if let Some(state) = self.recovery_schedules.get_mut(&id)
                && attempt_is_current
            {
                let current = state
                    .in_flight
                    .take()
                    .expect("the current recovery attempt must still be in flight");
                let deadline_reported = std::mem::take(&mut state.deadline_reported);
                let oversized_revision_is_current = state.revision == current.revision
                    && matches!(
                        &outcome,
                        CheckpointBatchOutcome::Failed(RecoveryError::OversizedCheckpoint { .. })
                    );
                if oversized_revision_is_current {
                    state.suppressed_oversized_revision = Some(current.revision);
                    state.protection_warning = true;
                } else if store_returned_at > current.timing.durable_complete_by {
                    if !deadline_reported {
                        state
                            .schedule
                            .checkpoint_deadline_missed(current.timing, store_returned_at);
                    }
                    state.protection_warning = true;
                } else {
                    match &outcome {
                        CheckpointBatchOutcome::Written => {
                            if deadline_reported && state.revision == current.revision {
                                state
                                    .schedule
                                    .mark_durable_baseline(current.timing.snapshot_at);
                            } else if !deadline_reported {
                                state.schedule.checkpoint_written(current.timing);
                            }
                            if state.revision == current.revision {
                                state.protection_warning = false;
                            }
                        }
                        CheckpointBatchOutcome::Superseded if !deadline_reported => {
                            if current.cancelled.load(Ordering::Acquire) {
                                state
                                    .schedule
                                    .checkpoint_cancelled(current.timing, store_returned_at);
                            } else {
                                state.schedule.checkpoint_superseded(current.timing);
                            }
                        }
                        CheckpointBatchOutcome::Failed(_) | CheckpointBatchOutcome::Deferred
                            if !deadline_reported =>
                        {
                            state
                                .schedule
                                .checkpoint_failed(current.timing, store_returned_at);
                            state.protection_warning = true;
                        }
                        CheckpointBatchOutcome::Superseded
                        | CheckpointBatchOutcome::Failed(_)
                        | CheckpointBatchOutcome::Deferred => {}
                    }
                }
            }
            match outcome {
                CheckpointBatchOutcome::Written if written_revision_is_current => {
                    log::debug!("recovery checkpoint written")
                }
                CheckpointBatchOutcome::Failed(error) if attempt_is_current => {
                    last_error = Some(error.to_string())
                }
                CheckpointBatchOutcome::Written
                | CheckpointBatchOutcome::Superseded
                | CheckpointBatchOutcome::Deferred
                | CheckpointBatchOutcome::Failed(_) => {}
            }
        }

        if let Some(status) = checkpoint_batch_status(maintenance_issues, last_error.as_deref()) {
            self.set_status(status, cx);
        }
        self.refresh_recovery_warning(cx);
        if worker_released {
            self.checkpoint_recovery_at(now, cx);
        } else {
            self.schedule_recovery_timer(cx);
        }
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
        let auto_reload = !self.startup_recovery_pending
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
            doc.update(cx, |doc, cx| {
                doc.save(SaveMode::Normal, cx);
            });
        }
    }

    fn on_save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(document) = self.active_document() {
            self.prompt_save_as(document.read(cx).id(), window, cx);
        }
    }

    fn prompt_save_as(
        &mut self,
        id: crate::lifecycle::DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.document_by_id(id, cx) else {
            return;
        };
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
                    this.finish_save_as_selection(id, path.clone(), window, cx);
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

    fn finish_save_as_selection(
        &mut self,
        id: crate::lifecycle::DocumentId,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match path {
            Some(path) => self.finish_save_as(id, path, SaveAsMode::CreateOnly, window, cx),
            None => self.cancel_pending_destructive_save_as(id),
        }
    }

    fn prompt_save_as_overwrite(
        &mut self,
        id: crate::lifecycle::DocumentId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let authorization = match fs::SaveAsOverwriteAuthorization::capture(&path) {
            Ok(authorization) => Arc::new(authorization),
            Err(error) => {
                self.cancel_pending_destructive_save_as(id);
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
                    if replace {
                        this.finish_save_as(
                            id,
                            path.clone(),
                            SaveAsMode::Overwrite(authorization.clone()),
                            window,
                            cx,
                        );
                    } else {
                        this.cancel_pending_destructive_save_as(id);
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

    fn finish_save_as(
        &mut self,
        id: crate::lifecycle::DocumentId,
        path: PathBuf,
        mode: SaveAsMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.document_index(id, cx) else {
            self.cancel_pending_destructive_save_as(id);
            return;
        };
        if !self.save_as_snapshot_is_current(id, cx) {
            self.cancel_pending_destructive_save_as(id);
            self.set_status(i18n::save_as_snapshot_changed_message(cx).into(), cx);
            return;
        }
        if self.tabs.iter().enumerate().any(|(existing, tab)| {
            existing != ix
                && tab
                    .path()
                    .is_some_and(|candidate| paths_match(candidate, &path))
        }) {
            self.cancel_pending_destructive_save_as(id);
            self.set_status(i18n::save_as_path_already_open_message(&path, cx), cx);
            return;
        }
        let document = self.document_at(ix).cloned().expect("index was found");
        let (old_path, old_recovery_key, saved_snapshot) = {
            let document = document.read(cx);
            (
                document.source_path().map(Path::to_path_buf),
                document.recovery_key(),
                document.source_snapshot(cx),
            )
        };
        let create_only = mode == SaveAsMode::CreateOnly;
        self.save_as_recovery_keys
            .insert(id, old_recovery_key.clone());
        match document.update(cx, |document, cx| document.save_as(&path, mode, cx)) {
            SaveAsOutcome::Saved => {
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
                self.complete_pending_destructive_save_as(
                    id,
                    saved_snapshot,
                    old_recovery_key,
                    window,
                    cx,
                );
                cx.notify();
            }
            SaveAsOutcome::DestinationExists if create_only => {
                self.save_as_recovery_keys.remove(&id);
                self.prompt_save_as_overwrite(id, path, window, cx);
            }
            SaveAsOutcome::DestinationExists | SaveAsOutcome::Failed => {
                self.save_as_recovery_keys.remove(&id);
                self.cancel_pending_destructive_save_as(id);
            }
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
            } else if crate::workspace::is_openable(path) {
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
        let _ = cx;
        self.tabs
            .menu_target()
            .and_then(|tab| tab.path().map(Path::to_path_buf))
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

    /// Translate `scope` of the active document.
    ///
    /// Runs on a background task: a network round-trip must never block the UI
    /// thread. The document engine decides what is translatable; this only
    /// picks the provider.
    fn translate(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        let settings = crate::settings::AppSettings::global(cx).clone();
        let Some(provider) = Provider::resolve(&settings) else {
            // Naming the fix rather than the symptom: "not configured" leaves
            // the user hunting through Settings for which field is missing.
            self.set_status(
                "No translation API key. Set one in Settings (Ctrl/Cmd+,), or export \
                 ANTHROPIC_API_KEY / OPENAI_API_KEY. A local server that wants no key \
                 still needs a placeholder."
                    .into(),
                cx,
            );
            return;
        };
        let service = match provider.build_with(&settings) {
            Ok(service) => service,
            Err(err) => {
                self.set_status(format!("Translation unavailable: {err}"), cx);
                return;
            }
        };

        let target = settings.translate_to.trim().to_string();
        let target = if target.is_empty() {
            "zh".to_string()
        } else {
            target
        };

        // Parse the editor's *current* text rather than reusing
        // `doc.document()`: that parse is debounced by 180ms, so translating
        // right after a keystroke would translate the previous text and then
        // overwrite the editor with it — silently discarding the edit.
        let source_snapshot = doc.read(cx).async_snapshot(cx);
        let text = source_snapshot.text().to_owned();
        let doc_type = doc.read(cx).document().doc_type();
        let doc = doc.downgrade();

        self.set_status(format!("Translating via {}…", provider.label()), cx);
        // Set before the spawn, not inside it: the button has to go inert on
        // this frame, or the second click lands before the task even starts.
        self.translating = true;

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let source = mt_doc::Document::with_type(doc_type, text);
                    mt_doc::translate::translate(&source, &scope, &target, service.as_ref())
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
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
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
                        Err(err) => this.set_status(format!("Translation failed: {err}"), cx),
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
                        .map(|root| crate::workspace::display_relative(root, path))
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
                .role(gpui::Role::DescriptionList)
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
        // The outline and the search results both land here, and both are the
        // kind of jump a user expects Back to undo.
        if let Some(path) = doc.read(cx).source_path() {
            self.record_visit(path.to_path_buf(), offset);
        }
        doc.update(cx, |doc, cx| doc.reveal_offset(offset, window, cx));
    }

    /// Open `path` (as a preview) and put the cursor at `offset`.
    ///
    /// The search-result path. A preview open because scanning a result list is
    /// exactly the browsing a preview tab exists for; a double click in the
    /// tree or an edit still promotes it.
    fn reveal_in(
        &mut self,
        path: PathBuf,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_as(path.clone(), true, window, cx);
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        // Guard against the open having failed — landing the cursor in whatever
        // tab happened to be active would be worse than doing nothing.
        if doc.read(cx).source_path() != Some(path.as_path()) {
            return;
        }
        self.record_visit(path, offset);
        doc.update(cx, |doc, cx| doc.reveal_offset(offset, window, cx));
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
        let add_open_tabs = |corpus: &mut Corpus| {
            for tab in self.tabs.iter() {
                let doc = tab.payload.view.read(cx);
                if let Some(path) = tab.path() {
                    corpus.open.push((path.to_path_buf(), doc.text(cx)));
                }
            }
        };

        match self.search.read(cx).scope() {
            Scope::Document => {
                if let Some(doc) = self.active_document() {
                    let doc = doc.read(cx);
                    if let Some(path) = doc.source_path() {
                        corpus.open.push((path.to_path_buf(), doc.text(cx)));
                    }
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
                let path = tab.path().map(Path::to_path_buf);
                let full = path
                    .as_ref()
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| "Unsaved document".to_string());
                // Relative only makes sense with a folder open, and only for a
                // file actually under it — a globally-discovered skill is not.
                let relative = path
                    .as_deref()
                    .zip(root.as_deref())
                    .and_then(|(path, root)| path.strip_prefix(root).ok())
                    .map(|rest| rest.to_string_lossy().replace('\\', "/"));
                let is_preview = path
                    .as_deref()
                    .is_some_and(|path| self.tabs.is_preview(path));
                let dirty = doc.is_dirty();
                let active_single_document = self.tabs.len() == 1 && self.tabs.active_index() == ix;
                let label = elide_tab_label(&doc.title(cx));
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
                    .when(!web_active && path.is_some(), |tab| {
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
                                .role(gpui::Role::Button)
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

    fn render_welcome(&self, cx: &Context<Self>) -> AnyElement {
        let recents = crate::settings::AppSettings::global(cx)
            .recent_targets
            .clone();
        let sample_available = self.welcome_sample_available;

        v_flex()
            .id("welcome")
            .role(gpui::Role::Group)
            .aria_label(i18n::t(i18n::Key::WelcomeTitle, cx))
            .size_full()
            .min_h_0()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(&self.welcome_scroll)
            .px_6()
            .py_8()
            .child(
                v_flex()
                    .w(px(560.))
                    .max_w_full()
                    .flex_shrink_0()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::BookOpen).large())
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(i18n::t(i18n::Key::WelcomeTitle, cx)),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(i18n::t(i18n::Key::WelcomeSubtitle, cx)),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Button::new("welcome-new")
                                    .icon(IconName::Plus)
                                    .label(i18n::t(i18n::Key::NewDocument, cx))
                                    .accessibility_id(WELCOME_NEW_ACCESSIBILITY_ID)
                                    .primary()
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_new_document(&NewDocument, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-paste")
                                    .icon(IconName::Copy)
                                    .label(i18n::t(i18n::Key::Paste, cx))
                                    .accessibility_id(WELCOME_PASTE_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_paste_into_new(&PasteIntoNew, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-file")
                                    .icon(IconName::File)
                                    .label(i18n::t(i18n::Key::OpenFilePicker, cx))
                                    .accessibility_id(WELCOME_OPEN_FILE_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_file(&OpenFile, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-folder")
                                    .icon(IconName::FolderOpen)
                                    .label(i18n::t(i18n::Key::OpenFolderPicker, cx))
                                    .accessibility_id(WELCOME_OPEN_FOLDER_ACCESSIBILITY_ID)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_folder(&OpenFolder, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-open-sample")
                                    .icon(IconName::BookOpen)
                                    .label(i18n::t(i18n::Key::OpenBundledSample, cx))
                                    .accessibility_id(WELCOME_OPEN_SAMPLE_ACCESSIBILITY_ID)
                                    .disabled(!sample_available)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_bundled_sample(window, cx);
                                    })),
                            ),
                    )
                    .when(!recents.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .mt_4()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(i18n::t(i18n::Key::Recent, cx)),
                                )
                                .children(recents.into_iter().map(|target| {
                                    let issue = self.welcome_recent_target_issue(&target);
                                    let path_text = target.path.to_string_lossy().into_owned();
                                    let label = if target.display_name.is_empty() {
                                        path_text.clone()
                                    } else {
                                        target
                                            .path
                                            .parent()
                                            .map(|parent| {
                                                format!(
                                                    "{}  {}",
                                                    target.display_name,
                                                    parent.display()
                                                )
                                            })
                                            .unwrap_or_else(|| target.display_name.clone())
                                    };
                                    let open_label =
                                        i18n::open_recent_target_label(&target.path, cx);
                                    let remove_label =
                                        i18n::remove_recent_target_label(&target.path, cx);
                                    let identity =
                                        RecoveryKey::for_path(&target.path).as_str().to_owned();
                                    let path = target.path.clone();
                                    let remove_path = path.clone();
                                    let open_id =
                                        SharedString::from(format!("welcome-recent-{identity}"));
                                    let open_accessibility_id = SharedString::from(format!(
                                        "markturbo-welcome-recent-{identity}"
                                    ));
                                    let remove_id = SharedString::from(format!(
                                        "welcome-recent-remove-{identity}"
                                    ));
                                    let status_id = SharedString::from(format!(
                                        "markturbo-welcome-recent-status-{identity}"
                                    ));
                                    let icon = match target.kind {
                                        crate::settings::RecentTargetKind::File => IconName::File,
                                        crate::settings::RecentTargetKind::Workspace => {
                                            IconName::Folder
                                        }
                                    };
                                    let open_button = if issue.is_some() {
                                        BaseButton::new(open_id)
                                            .role(gpui::Role::Button)
                                            .disabled(true)
                                            .accessibility_label(open_label)
                                            .accessibility_id(open_accessibility_id)
                                            .a11y_synthetic_children(|builder| {
                                                builder.parent_node().set_disabled();
                                            })
                                            .styles(|styles| {
                                                styles.disabled(|style| {
                                                    style
                                                        .bg(cx
                                                            .theme()
                                                            .input_background()
                                                            .opacity(0.5))
                                                        .border_color(cx.theme().input.opacity(0.5))
                                                        .text_color(
                                                            cx.theme()
                                                                .muted_foreground
                                                                .opacity(0.5),
                                                        )
                                                        .shadow_none()
                                                })
                                            })
                                            .flex()
                                            .flex_1()
                                            .min_w_0()
                                            .h_8()
                                            .px_2p5()
                                            .gap_2()
                                            .items_center()
                                            .justify_center()
                                            .rounded(cx.theme().radius)
                                            .border_1()
                                            .child(Icon::new(icon).small())
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .truncate()
                                                    .child(label.clone()),
                                            )
                                            .into_any_element()
                                    } else {
                                        Button::new(open_id)
                                            .icon(icon)
                                            .label(label.clone())
                                            .accessibility_label(open_label)
                                            .tooltip(path_text.clone())
                                            .accessibility_id(open_accessibility_id)
                                            .flex_1()
                                            .min_w_0()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_recent_target(&path, window, cx);
                                            }))
                                            .into_any_element()
                                    };
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .items_center()
                                        .child(open_button)
                                        .when_some(issue, move |this, issue| {
                                            let label = i18n::t(issue, cx);
                                            this.child(
                                                div()
                                                    .id(status_id.clone())
                                                    .role(gpui::Role::Label)
                                                    .aria_value(label)
                                                    .accessibility_id(status_id)
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(label),
                                            )
                                        })
                                        .child(
                                            Button::new(remove_id)
                                                .icon(IconName::Close)
                                                .accessibility_label(remove_label)
                                                .accessibility_id(SharedString::from(format!(
                                                    "markturbo-welcome-recent-remove-{identity}"
                                                )))
                                                .small()
                                                .ghost()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.remove_recent_target(&remove_path, cx);
                                                })),
                                        )
                                })),
                        )
                    })
                    .child(
                        Button::new("welcome-dont-show-again")
                            .label(i18n::t(i18n::Key::DontShowWelcomeAgain, cx))
                            .accessibility_id(WELCOME_DONT_SHOW_ACCESSIBILITY_ID)
                            .text()
                            .w_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dont_show_welcome_again(window, cx);
                            })),
                    ),
            )
            .into_any_element()
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
            (gpui::Role::Slider, gpui::accesskit::Orientation::Horizontal)
        } else {
            (gpui::Role::Splitter, gpui::accesskit::Orientation::Vertical)
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
            .on_a11y_action(gpui::accesskit::Action::Increment, move |_, window, cx| {
                if let Some(this) = increment.upgrade() {
                    this.update(cx, |this, cx| {
                        this.resize_panel_by_delta(edge, metrics::gap_group(), window, cx);
                    });
                }
            })
            .on_a11y_action(gpui::accesskit::Action::Decrement, move |_, window, cx| {
                if let Some(this) = decrement.upgrade() {
                    this.update(cx, |this, cx| {
                        this.resize_panel_by_delta(edge, -metrics::gap_group(), window, cx);
                    });
                }
            })
            .on_a11y_action(
                gpui::accesskit::Action::SetValue,
                move |data, window, cx| {
                    let Some(gpui::accesskit::ActionData::NumericValue(value)) = data else {
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
            .recovery_warning
            .clone()
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
        .role(gpui::Role::Group)
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
            .role(gpui::Role::Application)
            .aria_label("markturbo workspace")
            .track_focus(&self.focus_handle)
            .key_context(if self.show_welcome && !self.settings_open {
                WELCOME_KEY_CONTEXT
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
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewDocument, None),
        KeyBinding::new("ctrl-n", NewDocument, None),
        KeyBinding::new("cmd-v", PasteIntoNew, Some(WELCOME_KEY_CONTEXT)),
        KeyBinding::new("ctrl-v", PasteIntoNew, Some(WELCOME_KEY_CONTEXT)),
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

    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{
        DestructiveAction, DestructiveRequest, DestructiveResolution, DetailsContent,
        DirtyDecision, DocumentRecoveryState, RecoveryAttempt, RetirementCompletion, SaveAsMode,
        SaveAsOutcome, SaveMode, SidePanel, StartupRecovery, TAB_LABEL_MAX, Workspace,
        WorkspaceResizeEdge, checkpoint_batch_status, clamped_dragged_panel_width,
        current_checkpoint_write_completed, details_content, document_details_status_key,
        elide_tab_label, path_affects_harness, prepare_recovery_records,
        resolved_workspace_panel_widths, startup_recovery_status,
    };
    use crate::fs::{FileStamp, Newline, SourceIdentity};
    use crate::i18n;
    use crate::recovery::{
        CheckpointBatchOutcome, CheckpointOutcome, CheckpointSchedule, RecoveredRecord,
        RecoveryCheckpoint, RecoveryError, RecoveryIssue, RecoveryKey, RecoveryLimits,
        RecoveryMaintenance, RecoveryMetadata, RecoveryProtector, RecoveryRecord, RecoveryScan,
        RecoveryStore, RecoveryToken,
    };
    use crate::views::Layout;
    use crate::watcher::Change;
    use crate::web::{self, Trust};
    use gpui::{
        AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Modifiers, MouseButton,
        TestAppContext, VisualTestContext, Window, point, px,
    };

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
        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, initial, StartupRecovery::default);
        cx.run_until_parked();
        let recovery_root = tempfile::tempdir().unwrap();
        let recovery = RecoveryStore::new_at(
            recovery_root.path().join("store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        workspace.update(cx, |workspace, _| {
            workspace.startup_recovery_pending = false;
            workspace.recovery = Some(recovery);
            workspace._test_recovery_root = Some(recovery_root);
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
            gpui_component::init(cx);
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
                gpui_component::Root::new(workspace, window, cx)
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

    fn open_test_workspace_with_welcome_preference(
        cx: &mut TestAppContext,
        show_welcome_on_startup: bool,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
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
                gpui_component::Root::new(workspace, window, cx)
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
        let loaded = crate::fs::load(path).unwrap();
        let checkpoint = RecoveryCheckpoint {
            key: RecoveryKey::for_path(path),
            text: text.to_string(),
            metadata: RecoveryMetadata::from_loaded_file(&loaded),
        };
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();
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
                recovery: workspace.recovery.clone(),
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
            revision,
            timing,
            cancelled,
        }
    }

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[test]
    fn welcome_visibility_requires_a_no_argument_launch_and_the_saved_preference() {
        assert!(super::should_show_welcome(None, true));
        assert!(!super::should_show_welcome(
            Some(Path::new("workspace")),
            true
        ));
        assert!(!super::should_show_welcome(None, false));
    }

    #[gpui::test]
    fn no_argument_workspace_starts_on_the_welcome_state(cx: &mut TestAppContext) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.show_welcome);
            assert!(workspace.tabs.is_empty());
            assert!(workspace.root.is_none());
        });
    }

    #[gpui::test]
    fn explicit_path_bypasses_welcome_and_records_its_file_target(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opened.md");
        fs::write(&path, "# Opened\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert_eq!(workspace.root.as_deref(), path.parent());
            assert_eq!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .first()
                    .map(|target| target.path.as_path()),
                Some(path.as_path())
            );
        });
    }

    #[gpui::test]
    fn paste_creates_an_exact_dirty_memory_document(cx: &mut TestAppContext) {
        let text = "# \u{7cbe}\u{8d34} \u{1f680}\nexact clipboard text\n";
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            workspace.update(app, |workspace, cx| {
                workspace.on_paste_into_new(&super::PasteIntoNew, window, cx);
            });
        });
        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert_eq!(document.layout(), Layout::Source);
        });
    }

    #[gpui::test]
    fn unavailable_welcome_clipboard_preserves_the_surface_and_reports_the_reason(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_paste_into_new(&super::PasteIntoNew, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.show_welcome);
            assert!(workspace.root.is_none());
            assert!(workspace.tabs.is_empty());
            assert_eq!(
                workspace.status.as_deref(),
                Some(i18n::t(i18n::Key::ClipboardTextUnavailable, app))
            );
        });
    }

    #[gpui::test]
    fn welcome_ctrl_v_pastes_into_a_new_document(cx: &mut TestAppContext) {
        let text = "# Clipboard shortcut\nexact \u{4e2d}\u{6587} \u{1f680}\n";
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));

        cx.simulate_keystrokes("ctrl-v");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
        });
    }

    #[gpui::test]
    fn welcome_paste_shortcut_is_inactive_while_settings_is_visible(cx: &mut TestAppContext) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string("settings input".to_string()));
            workspace.update(app, |workspace, cx| {
                workspace.on_open_settings(&super::OpenSettings, window, cx);
            });
        });

        cx.simulate_keystrokes("ctrl-v");
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.settings_open);
            assert!(workspace.show_welcome);
            assert!(workspace.tabs.is_empty());
        });
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn failed_file_open_keeps_welcome_root_tabs_and_focus_unchanged(cx: &mut TestAppContext) {
        use std::os::windows::fs::OpenOptionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.md");
        fs::write(&path, "locked\n").unwrap();
        let _lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

        let opened = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file_target(path.clone(), window, cx)
            })
        });

        assert!(!opened);
        cx.update(|window, app| {
            let workspace = workspace.read(app);
            assert!(workspace.show_welcome);
            assert!(workspace.root.is_none());
            assert!(workspace.tabs.is_empty());
            assert!(workspace.focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn cancelling_file_and_folder_pickers_preserves_the_welcome_state_and_focus(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_open_file(&super::OpenFile, window, cx);
            });
        });
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files);
            assert!(!options.directories);
            None
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.on_open_folder(&super::OpenFolder, window, cx);
            });
        });
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(!options.files);
            assert!(options.directories);
            None
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            let workspace = workspace.read(app);
            assert!(workspace.show_welcome);
            assert!(workspace.root.is_none());
            assert!(workspace.tabs.is_empty());
            assert!(workspace.status.is_none());
            assert!(workspace.focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn bundled_sample_opens_from_welcome_and_becomes_the_recent_workspace(cx: &mut TestAppContext) {
        let sample = crate::app_paths::bundled_sample_dir().expect("the debug sample");
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_bundled_sample(window, cx);
            });
        });

        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert_eq!(workspace.root.as_deref(), Some(sample.as_path()));
            let recent = crate::settings::AppSettings::global(app)
                .recent_targets
                .first()
                .expect("the sample recent target");
            assert_eq!(recent.path, sample);
            assert_eq!(recent.kind, crate::settings::RecentTargetKind::Workspace);
        });
    }

    #[gpui::test]
    fn missing_recent_target_is_disabled_and_removable_without_opening_anything(
        cx: &mut TestAppContext,
    ) {
        let missing = PathBuf::from("Q:/definitely/not/here/markturbo-missing.md");
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.record_recent_target(crate::settings::RecentTarget::new(
                    missing.clone(),
                    crate::settings::RecentTargetKind::File,
                    "missing.md",
                ));
            });
            workspace.update(app, |workspace, cx| {
                assert!(!workspace.open_recent_target(&missing, window, cx));
                workspace.remove_recent_target(&missing, cx);
            });
        });
        workspace.read_with(cx, |workspace, app| {
            assert!(workspace.tabs.is_empty());
            assert!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .is_empty()
            );
        });
    }

    #[test]
    fn recent_target_validation_distinguishes_missing_and_mismatched_entries() {
        let dir = tempfile::tempdir().unwrap();
        let directory = crate::settings::RecentTarget::new(
            dir.path(),
            crate::settings::RecentTargetKind::File,
            "directory",
        );
        assert_eq!(
            super::recent_target_issue(&directory),
            Some(i18n::Key::RecentUnavailable)
        );
        let missing = crate::settings::RecentTarget::new(
            dir.path().join("missing.md"),
            crate::settings::RecentTargetKind::File,
            "missing.md",
        );
        assert_eq!(
            super::recent_target_issue(&missing),
            Some(i18n::Key::RecentMissing)
        );
    }

    #[test]
    fn stale_recent_button_reports_the_accesskit_disabled_state() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let welcome = source
            .split_once("fn render_welcome")
            .expect("Welcome renderer")
            .1
            .split_once("fn render_status_bar")
            .expect("end of Welcome renderer")
            .0;

        assert!(welcome.contains("BaseButton::new(open_id)"));
        assert!(welcome.contains(".disabled(true)"));
        assert!(welcome.contains(".a11y_synthetic_children("));
        assert!(welcome.contains("builder.parent_node().set_disabled()"));
    }

    #[gpui::test]
    fn valid_recent_file_reopens_through_the_shared_target_path(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent.md");
        let text = "# Recent \u{4e2d}\u{6587} \u{1f680}\nexact text\n";
        fs::write(&path, text).unwrap();
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            crate::settings::AppSettings::update(app, |settings| {
                settings.record_recent_target(crate::settings::RecentTarget::new(
                    path.clone(),
                    crate::settings::RecentTargetKind::File,
                    "recent.md",
                ));
            });
            workspace.update(app, |workspace, cx| {
                assert!(workspace.open_recent_target(&path, window, cx));
            });
        });
        workspace.read_with(cx, |workspace, app| {
            assert_eq!(workspace.root.as_deref(), path.parent());
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(workspace.document_at(0).unwrap().read(app).text(app), text);
            assert_eq!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .first()
                    .map(|target| target.path.as_path()),
                Some(path.as_path())
            );
        });
    }

    #[gpui::test]
    fn dont_show_welcome_again_persists_and_starts_an_empty_memory_document(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.dont_show_welcome_again(window, cx);
            });
        });
        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert!(!crate::settings::AppSettings::global(app).show_welcome_on_startup);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), None);
            assert_eq!(document.text(app), "");
            assert!(!document.is_dirty());
        });
    }

    #[gpui::test]
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

    #[test]
    fn window_close_drains_input_then_reenters_the_native_close_path() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let request = source
            .split_once("fn request_window_close")
            .expect("window close request")
            .1
            .split_once("fn close_window_after_input_drain")
            .expect("end of window close request")
            .0;
        assert!(request.contains("if self.window_close_ready"));
        assert!(request.contains("return true;"));

        let close = source
            .split_once("fn close_window_after_input_drain")
            .expect("window close helper")
            .1
            .split_once("fn request_workspace_replace")
            .expect("end of window close helper")
            .0;
        assert!(close.contains("window.disable_focus(cx)"));
        assert_eq!(close.matches("window.on_next_frame").count(), 2);
        assert!(close.contains("workspace.window_close_ready = true"));
        assert!(close.contains("Self::post_native_window_close(window)"));

        let native = source
            .split_once("fn post_native_window_close")
            .expect("native close helper")
            .1
            .split_once("fn request_workspace_replace")
            .expect("end of native close helper")
            .0;
        assert!(native.contains("PostMessageW"));
        assert!(native.contains("WM_CLOSE"));
    }

    #[gpui::test]
    fn disabled_welcome_starts_future_no_argument_workspaces_with_a_new_buffer(
        cx: &mut TestAppContext,
    ) {
        let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, false);
        workspace.read_with(cx, |workspace, app| {
            assert!(!workspace.show_welcome);
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.source_path(), None);
            assert_eq!(document.text(app), "");
            assert!(!document.is_dirty());
        });
    }

    #[test]
    fn welcome_openers_keep_picker_cancel_and_drag_drop_on_one_target_path() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let file = source
            .split_once("fn on_open_file")
            .expect("file picker")
            .1
            .split_once("fn on_open_folder")
            .unwrap()
            .0;
        assert!(file.contains("files: true"));
        assert!(file.contains("directories: false"));
        assert!(file.contains("else {\n                return;"));
        assert!(file.contains("this.open_target(path, true, window, cx);"));
        let folder = source
            .split_once("fn on_open_folder")
            .expect("folder picker")
            .1
            .split_once("fn on_save")
            .unwrap()
            .0;
        assert!(folder.contains("files: false"));
        assert!(folder.contains("directories: true"));
        assert!(folder.contains("else {\n                return;"));
        assert!(folder.contains("this.open_target(path, true, window, cx);"));
        let drop = source
            .split_once("fn on_drop_paths")
            .expect("drop handler")
            .1
            .split_once("/// The path of whichever")
            .unwrap()
            .0;
        assert!(drop.contains("self.open_target(path.clone(), true, window, cx)"));
        assert!(source.contains("fn open_bundled_sample"));
        assert!(source.contains("crate::app_paths::bundled_sample_dir()"));
    }

    #[test]
    fn welcome_controls_have_stable_windows_uia_ids() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        for id in [
            "markturbo-welcome-new",
            "markturbo-welcome-paste",
            "markturbo-welcome-open-file",
            "markturbo-welcome-open-folder",
            "markturbo-welcome-open-sample",
            "markturbo-welcome-dont-show-again",
        ] {
            assert!(source.contains(id), "missing {id}");
        }
        assert!(source.contains(".accessibility_id("));
        assert!(source.contains("markturbo-welcome-recent-status-"));
        assert!(source.contains(".role(gpui::Role::Label)"));
        assert!(source.contains(".aria_value(label)"));
        let welcome = source
            .split_once("fn render_welcome")
            .expect("welcome renderer")
            .1
            .split_once("fn render_web_path_controls")
            .unwrap()
            .0;
        assert!(welcome.contains("RecoveryKey::for_path(&target.path)"));
        assert!(
            welcome.contains(".overflow_y_scroll()"),
            "ten recent entries must remain reachable at the minimum window height"
        );
        assert!(
            !welcome.contains("recents.into_iter().enumerate()"),
            "recent control identity must follow the path, not its MRU index"
        );
    }

    #[test]
    fn welcome_render_uses_cached_filesystem_availability() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let welcome = source
            .split_once("fn render_welcome")
            .expect("welcome renderer")
            .1
            .split_once("fn render_web_path_controls")
            .unwrap()
            .0;

        assert!(welcome.contains("self.welcome_sample_available"));
        assert!(welcome.contains("self.welcome_recent_target_issue(&target)"));
        assert!(
            !welcome.contains("crate::app_paths::bundled_sample_dir()")
                && !welcome.contains("let issue = recent_target_issue(&target);"),
            "Welcome rendering must not synchronously probe the filesystem"
        );
    }

    #[test]
    fn welcome_and_save_as_copy_names_the_target_and_follow_up_choice() {
        let workspace = crate::views::production_source(include_str!("workspace.rs"));
        let welcome = workspace
            .split_once("fn render_welcome")
            .expect("welcome renderer")
            .1
            .split_once("fn render_web_path_controls")
            .unwrap()
            .0;
        assert!(welcome.contains("Key::OpenFilePicker"));
        assert!(welcome.contains("Key::OpenFolderPicker"));
        assert!(welcome.contains("i18n::open_recent_target_label"));
        assert!(welcome.contains("i18n::remove_recent_target_label"));
        assert!(welcome.contains(".tooltip(path_text.clone())"));

        let overwrite = workspace
            .split_once("fn prompt_save_as_overwrite")
            .expect("overwrite prompt")
            .1
            .split_once("fn finish_save_as")
            .unwrap()
            .0;
        assert!(overwrite.contains("i18n::replace_file_title"));
        assert!(overwrite.contains("i18n::replace_file_description"));
        assert!(!overwrite.contains("\"Replace existing file?\""));

        let finish = workspace
            .split_once("fn finish_save_as")
            .expect("Save As completion")
            .1
            .split_once("fn on_close_tab")
            .unwrap()
            .0;
        assert!(finish.contains("i18n::save_as_snapshot_changed_message"));
        assert!(finish.contains("i18n::save_as_path_already_open_message"));

        let document = crate::views::production_source(include_str!("document.rs"));
        assert!(document.contains("Key::SaveAsPicker"));
    }

    #[test]
    fn welcome_hides_document_only_title_commands() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let left = source
            .split_once("fn render_left_toggle_overlay")
            .expect("left title control")
            .1
            .split_once("fn render_title_commands_overlay")
            .unwrap()
            .0;
        assert!(left.contains(".when(!self.show_welcome"));

        let commands = source
            .split_once("fn render_title_commands")
            .expect("title commands")
            .1
            .split_once("fn render_left_toggle_overlay")
            .unwrap()
            .0;
        assert!(commands.contains(".when(!self.show_welcome"));
        assert!(commands.contains("i18n::Key::Settings"));
    }

    #[gpui::test]
    fn ten_recent_targets_scroll_into_view_at_the_minimum_window_size(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let mut targets = Vec::new();
        for ix in 0..10 {
            let path = dir.path().join(format!(
                "{ix:02}-a-very-long-recent-document-name-for-layout-\u{4e2d}\u{6587}.md"
            ));
            if ix < 8 {
                fs::write(&path, format!("# Recent {ix}\n")).unwrap();
            }
            targets.push(path);
        }

        cx.update(|app| {
            gpui_component::init(app);
            crate::settings::AppSettings::init(app);
            super::init(app);
        });
        let captured = Rc::new(RefCell::new(None));
        let window = cx.open_window(gpui::size(px(720.), px(480.)), {
            let captured = captured.clone();
            move |window, app| {
                let workspace = app.new(|cx| {
                    Workspace::new_with_startup_recovery(None, StartupRecovery::default, window, cx)
                });
                *captured.borrow_mut() = Some(workspace.clone());
                gpui_component::Root::new(workspace, window, app)
            }
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let workspace = captured.borrow().clone().expect("the Workspace entity");
        cx.update(|window, app| {
            crate::settings::AppSettings::update(app, |settings| {
                for path in &targets {
                    settings.record_recent_target(crate::settings::RecentTarget::new(
                        path.clone(),
                        crate::settings::RecentTargetKind::File,
                        path.file_name().unwrap().to_string_lossy(),
                    ));
                }
            });
            let handle = workspace.read(app).focus_handle(app);
            window.focus(&handle, app);
            window.draw(app).clear(app);
        });
        cx.run_until_parked();
        cx.update(|window, app| window.draw(app).clear(app));

        let (before, max, bounds) = workspace.read_with(&cx, |workspace, _| {
            (
                workspace.welcome_scroll.offset(),
                workspace.welcome_scroll.max_offset(),
                workspace.welcome_scroll.bounds(),
            )
        });
        assert!(
            max.y > px(0.),
            "ten recent targets must overflow vertically"
        );
        assert!(bounds.top() >= crate::metrics::title_bar());
        assert!(bounds.bottom() <= px(480.) - crate::metrics::status_bar());

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(360.), px(240.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-2_000.))),
            ..Default::default()
        });
        cx.update(|window, app| window.draw(app).clear(app));

        let after = workspace.read_with(&cx, |workspace, _| workspace.welcome_scroll.offset());
        assert!(
            after.y < before.y,
            "the welcome page must respond to scrolling"
        );
        assert_eq!(after.y, -max.y, "the full recent list must be reachable");
    }

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            assert!(workspace.recovery_schedules.contains_key(&id));
        });
    }

    #[gpui::test]
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
            assert!(workspace.recovery_schedules.contains_key(&id));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), text);
            assert!(document.is_dirty());
            assert_eq!(document.source_path(), None);
        });
    }

    #[gpui::test]
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
            assert!(!workspace.recovery_schedules.contains_key(&id));
            assert_eq!(
                crate::settings::AppSettings::global(app)
                    .recent_targets
                    .first()
                    .map(|target| target.path.as_path()),
                Some(destination.as_path())
            );
        });
    }

    #[gpui::test]
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
            assert_eq!(recent.kind, crate::settings::RecentTargetKind::Workspace);
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
                !workspace.recovery_schedules.contains_key(&document.id()),
                "a clean dropped file must not enter dirty-buffer recovery"
            );
            let recents = &crate::settings::AppSettings::global(app).recent_targets;
            assert_eq!(recents[0].path, document_path);
            assert_eq!(recents[0].kind, crate::settings::RecentTargetKind::File);
            assert_eq!(recents[1].path, folder.path());
            assert_eq!(
                recents[1].kind,
                crate::settings::RecentTargetKind::Workspace
            );
        });
    }

    #[gpui::test]
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
            assert!(workspace.recovery_schedules.contains_key(&id));
            let document = workspace.document_at(0).unwrap().read(app);
            assert_eq!(document.text(app), revised);
            assert!(document.is_dirty());
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace_with(cx, None);
        workspace.update(cx, |workspace, _| workspace.recovery = Some(store));

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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
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
            assert!(workspace.recovery_retirement_batches.contains_key(&key));
            assert!(!workspace.pending_recovery_retirements.contains_key(&key));
            assert!(
                !workspace.recovery_schedules.contains_key(&id),
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

    #[gpui::test]
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
                workspace.recovery = None;
                workspace.startup_recovery_pending = true;
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

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
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

    #[gpui::test]
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
                workspace.recovery = None;
                workspace.startup_recovery_pending = true;
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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace_with(cx, None);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.recovery = Some(store.clone());
                workspace.open_file_as(first.clone(), true, window, cx);
            });
        });
        let key = RecoveryKey::for_path(&first);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace.recovery_retirements.insert(key.clone(), old);
            workspace.pending_recovery_retirements.insert(key, None);
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

    #[gpui::test]
    fn unavailable_startup_store_keeps_waiting_discard_open(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-without-store.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
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
            assert!(!workspace.startup_recovery_pending);
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
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

    #[gpui::test]
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
    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            assert_eq!(document.document().doc_type(), mt_doc::DocType::Html);
            assert_eq!(document.text(app), text);
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui::test]
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
            assert_eq!(document.document().doc_type(), mt_doc::DocType::Html);
            assert_eq!(document.trust(), Trust::Restricted);
            let payload = document.web_html().expect("the rebuilt HTML payload");
            assert!(!payload.starts_with("file://"));
            assert!(web::to_data_url(payload).starts_with("data:text/html;charset=utf-8,"));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui::test]
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
            assert_eq!(document.document().doc_type(), mt_doc::DocType::Html);
            assert_eq!(document.trust(), Trust::Restricted);
            let payload = document.web_html().expect("the rebuilt HTML payload");
            assert!(!payload.starts_with("file://"));
            assert!(web::to_data_url(payload).starts_with("data:text/html;charset=utf-8,"));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), text);
    }

    #[gpui::test]
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

    #[gpui::test]
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
            assert_eq!(document.document().doc_type(), mt_doc::DocType::Markdown);
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

    #[gpui::test]
    fn discard_keeps_the_tab_open_when_recovery_retirement_fails(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-retirement-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let edited = "discard only after durable retirement\n";
        replace_document(&workspace, 0, edited, cx);
        write_recovery_checkpoint(&store, &path, "older checkpoint\n");
        let now = cx.background_executor.now();
        workspace.update(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
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
                revision: document.revision(),
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

    #[gpui::test]
    fn discard_proceeds_after_the_record_is_retired_even_if_cleanup_fails(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discard-cleanup-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
    fn save_retries_a_failed_durable_recovery_retirement(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save-retirement-retry.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let edited = "saved before retirement retry\n";
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
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
            assert!(
                workspace
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
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
            assert!(
                !workspace
                    .recovery_retirement_retries
                    .contains(&checkpoint.key)
            );
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
                workspace.pending_recovery_retirements.get(&checkpoint.key),
                Some(&Some(first_id))
            );
            assert!(
                workspace
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
                    .pending_recovery_retirements
                    .contains_key(&checkpoint.key)
            );
        });
    }

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
            workspace
                .startup_recovery_keys
                .insert(id, original_key.clone());
        });
        replace_document(&workspace, 0, "saved elsewhere\n", cx);
        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.finish_save_as(id, saved_as.clone(), SaveAsMode::CreateOnly, window, cx);
            });
        });

        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.pending_recovery_retirements.get(&original_key),
                Some(&Some(id))
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
                .expect("the original startup key must delay the target tab close");
            assert!(pending.keys.contains(&(original_key, Some(id))));
        });
        assert_eq!(fs::read_to_string(saved_as).unwrap(), "saved elsewhere\n");
    }

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
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
                workspace.startup_recovery_keys.get(&id),
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
                workspace.pending_recovery_retirements.get(&original_key),
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
                assert!(workspace.recovery_retirements.contains_key(&original_key));
            });
        });
        cx.run_until_parked();

        assert!(store.recover().unwrap().records.is_empty());
        assert_eq!(
            fs::read_to_string(saved_as).unwrap(),
            "disk before startup\n"
        );
    }

    #[gpui::test]
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
                    .pending_recovery_retirements
                    .insert(first_key.clone(), Some(first_id));
                workspace
                    .pending_recovery_retirements
                    .insert(second_key.clone(), Some(second_id));
                workspace
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
                    .recovery_retirement_batches
                    .get(&first_key)
                    .cloned()
                    .expect("the first pending key must enter the batch");
                assert_eq!(
                    workspace.recovery_retirement_batches.get(&second_key),
                    Some(&batch)
                );
                assert_eq!(
                    workspace.recovery_retirement_batches.get(&unknown_key),
                    Some(&batch)
                );
                assert_eq!(workspace.recovery_retirement_batches.len(), 3);
            });
        });
    }

    #[gpui::test]
    fn second_save_replaces_a_stale_ui_retirement_owner(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("second-save-stale-owner.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        write_recovery_checkpoint(&store, &path, "first saved checkpoint\n");
        let key = RecoveryKey::for_path(&path);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_retirements
                .insert(key.clone(), old.clone());
        });
        let old_completion = store.complete_retirement(old.clone()).unwrap();

        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, None, cx);
        });
        let fresh = workspace.read_with(cx, |workspace, _| {
            let fresh = workspace
                .recovery_retirements
                .get(&key)
                .cloned()
                .expect("the second Save must install a fresh retirement owner");
            assert_ne!(fresh, old);
            assert!(!workspace.pending_recovery_retirements.contains_key(&key));
            fresh
        });

        workspace.update(cx, |workspace, cx| {
            workspace.finish_recovery_retirement(key.clone(), old, None, Ok(old_completion), cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_retirements.get(&key), Some(&fresh));
            assert!(!workspace.pending_recovery_retirements.contains_key(&key));
        });
    }

    #[gpui::test]
    fn matched_old_completion_replays_a_queued_save_retirement(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queued-save-replay.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        write_recovery_checkpoint(&store, &path, "queued checkpoint\n");
        let key = RecoveryKey::for_path(&path);
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
                .recovery_retirements
                .insert(key.clone(), old.clone());
        });

        workspace.update(cx, |workspace, cx| {
            workspace.invalidate_recovery(&key, None, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_retirements.get(&key), Some(&old));
            assert!(workspace.pending_recovery_retirements.contains_key(&key));
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
                .recovery_retirements
                .get(&key)
                .expect("the matched old completion must replay the queued Save");
            assert_ne!(fresh, &old);
            assert!(!workspace.pending_recovery_retirements.contains_key(&key));
        });
    }

    #[gpui::test]
    fn edit_cancels_only_the_queued_retirement_intent(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edit-cancels-queued-retirement.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        let key = document.read_with(cx, |document, _| document.recovery_key());
        let old = store.begin_retirement(&key).unwrap();
        workspace.update(cx, |workspace, _| {
            workspace
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
            assert!(!workspace.pending_recovery_retirements.contains_key(&key));
            assert_eq!(workspace.recovery_retirements.get(&key), Some(&old));
        });
    }

    #[gpui::test]
    fn stale_batch_takeover_resumes_the_destructive_action(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-batch-takeover.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
    fn replay_persist_failure_keeps_destructive_action_open_until_retry(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replay-persist-failure.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
                    .recovery_retirement_batches
                    .insert(key.clone(), old_batch.clone());
                workspace
                    .pending_recovery_retirements
                    .insert(key.clone(), Some(id));
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
            assert!(workspace.pending_recovery_retirements.contains_key(&key));
            assert!(workspace.pending_startup_destructive.is_some());
        });

        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        assert!(workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
            assert!(workspace.recovery_retirement_retries.contains(&first_key));
            assert!(
                workspace
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
                workspace.pending_recovery_retirements.remove(&first_key);
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
                workspace.recovery_retirement_batches.is_empty(),
                "the old marker retry must not strand a later batch cleanup"
            );
            assert!(workspace.pending_recovery_retirements.is_empty());
            assert!(workspace.recovery_retirement_retries.is_empty());
        });

        replace_document(&workspace, 0, "dirty after cleanup\n", cx);
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, _| workspace.tabs.is_empty()));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[gpui::test]
    fn save_and_discard_clear_the_recovery_record(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

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

    #[gpui::test]
    fn save_and_discard_supersede_an_in_flight_checkpoint(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in-flight.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

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
                let state = workspace.recovery_schedules.get(&document.id()).unwrap();
                let attempt = test_recovery_attempt(
                    state
                        .token
                        .clone()
                        .expect("a ready test store must provide a recovery token"),
                    state.revision,
                    Instant::now(),
                    Arc::new(AtomicBool::new(false)),
                );
                (document.id(), document.recovery_checkpoint(app), attempt)
            });
            store
                .checkpoint(&checkpoint, &HashSet::from([checkpoint.key.clone()]))
                .unwrap();
            workspace.update(cx, |workspace, _| {
                workspace.recovery_schedules.get_mut(&id).unwrap().in_flight =
                    Some(attempt.clone());
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

    #[gpui::test]
    fn clean_and_closed_documents_retire_recovery_deadlines(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deadline.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store);
        });

        replace_document(&workspace, 0, "save me\n", cx);
        let generation_before_save = workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_schedules.len(), 1);
            assert!(workspace._recovery_timer.is_some());
            workspace.recovery_timer_generation
        });
        let document = workspace
            .read_with(cx, |workspace, _| workspace.document_at(0).cloned())
            .unwrap();
        document.update(cx, |document, cx| {
            assert!(document.save(SaveMode::Normal, cx));
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_schedules.is_empty());
            assert!(workspace._recovery_timer.is_none());
            assert!(workspace.recovery_timer_generation > generation_before_save);
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open_file(path, window, cx);
            });
        });
        replace_document(&workspace, 0, "discard me\n", cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_schedules.len(), 1);
            assert!(workspace._recovery_timer.is_some());
        });
        cx.simulate_keystrokes("ctrl-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_schedules.is_empty());
            assert!(workspace._recovery_timer.is_none());
        });
    }

    #[gpui::test]
    fn stale_recovery_completion_cannot_clear_a_newer_attempt(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-completion.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        let (id, key) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            (document.id(), document.recovery_key())
        });
        let old_token = store.current_token(&key);
        store.invalidate_and_delete(&key).unwrap();
        let new_token = store.current_token(&key);
        let now = Instant::now();
        let mut schedule = super::CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let timing = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        let attempt = super::RecoveryAttempt {
            token: new_token.clone(),
            revision: 7,
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let deadline = schedule.next_deadline();

        workspace.update(cx, |workspace, cx| {
            workspace.recovery = Some(store);
            workspace.recovery_schedules.insert(
                id,
                super::DocumentRecoveryState {
                    key,
                    revision: 7,
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert_eq!(state.schedule.next_deadline(), deadline);
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first_path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
                    revision: document.revision(),
                    timing: schedule.checkpoint_dispatched(now).unwrap(),
                    cancelled: if id == first_id {
                        first_cancelled.clone()
                    } else {
                        second_cancelled.clone()
                    },
                };
                workspace.recovery_schedules.insert(
                    id,
                    DocumentRecoveryState {
                        key: document.recovery_key(),
                        revision: document.revision(),
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
            let first = workspace.recovery_schedules.get(&first_id).unwrap();
            let second = workspace.recovery_schedules.get(&second_id).unwrap();
            assert!(first.in_flight.is_some());
            assert!(second.in_flight.is_some());
            assert!(!Arc::ptr_eq(
                &first.in_flight.as_ref().unwrap().cancelled,
                &second.in_flight.as_ref().unwrap().cancelled,
            ));
        });
    }

    #[gpui::test]
    fn cancelled_checkpoint_catches_up_after_the_retry_throttle(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancelled-catch-up.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        replace_document(&workspace, 0, "first snapshot\n", cx);

        let now = cx.background_executor.now();
        let (id, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(now);
            (
                document.id(),
                RecoveryAttempt {
                    token: state
                        .token
                        .clone()
                        .expect("a ready test store must provide a recovery token"),
                    revision: document.revision(),
                    timing: schedule.checkpoint_dispatched(now).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get_mut(&id).unwrap();
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
            assert!(workspace.recovery_warning.is_none());
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(edited_at);
            (
                document.id(),
                document.recovery_checkpoint(app),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    revision: document.revision(),
                    timing: schedule
                        .checkpoint_dispatched(edited_at + Duration::from_secs(2))
                        .unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get_mut(&id).unwrap();
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
            workspace.recovery_checkpoint_worker_active = true;
            workspace._recovery_timer = None;
        });

        let (worker_paused, release_worker) = store.pause_after_checkpoint_final_check_for_test();
        let worker_store = store.clone();
        let worker_checkpoint = checkpoint.clone();
        let worker_attempt = attempt.clone();
        let worker_key = checkpoint.key.clone();
        let worker = std::thread::spawn(move || {
            worker_store.checkpoint_batch_if_current_cancellable(
                [crate::recovery::CancellableRecoveryCheckpointAttempt {
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

    #[gpui::test]
    fn checkpoint_returning_after_its_durable_deadline_keeps_the_warning(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store-late.md");
        fs::write(&path, "disk\n").unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        replace_document(&workspace, 0, "late checkpoint\n", cx);
        let dispatched_at = cx.background_executor.now();
        let (id, attempt) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            (
                document.id(),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    revision: document.revision(),
                    timing: schedule.checkpoint_dispatched(dispatched_at).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get_mut(&id).unwrap();
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(state.schedule.next_deadline().is_some());
            assert!(workspace.recovery_warning.is_some());
        });
    }

    #[gpui::test]
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
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
            let mut schedule = CheckpointSchedule::default();
            schedule.mark_dirty(dispatched_at);
            (
                document.id(),
                RecoveryAttempt {
                    token: state.token.clone().unwrap(),
                    revision: document.revision(),
                    timing: schedule.checkpoint_dispatched(dispatched_at).unwrap(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            )
        });
        workspace.update(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get_mut(&id).unwrap();
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert!(!state.protection_warning);
            assert!(workspace.recovery_warning.is_none());
        });
    }

    #[gpui::test]
    fn overdue_checkpoint_waits_for_the_physical_worker_before_retrying(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overdue-stuck.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let latest = "latest while stale worker is stuck\n";
        replace_document(&workspace, 0, latest, cx);

        let now = cx.background_executor.now();
        let (id, token, revision) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
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
            revision,
            timing: schedule.checkpoint_dispatched(now).unwrap(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get_mut(&id).unwrap();
            state.schedule = schedule;
            state.in_flight = Some(attempt.clone());
            state.deadline_reported = false;
            workspace.recovery_checkpoint_worker_active = true;
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(state.deadline_reported);
            assert!(state.protection_warning);
            assert!(workspace._recovery_timer.is_none());
        });
        assert!(attempt.cancelled.load(Ordering::Acquire));

        cx.background_executor.advance_clock(Duration::from_secs(1));
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(workspace.recovery_warning.is_some());
        });
        assert!(store.recover().unwrap().records.is_empty());
        workspace.update(cx, |workspace, _| {
            workspace.recovery_checkpoint_worker_active = false;
            workspace.recovery_schedules.remove(&id);
            workspace._recovery_timer = None;
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        replace_document(&workspace, 0, "protected late\n", cx);

        let now = cx.background_executor.now();
        let (id, key, revision, token) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
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
            revision,
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, _| {
            workspace.recovery_schedules.insert(
                id,
                DocumentRecoveryState {
                    key,
                    revision,
                    suppressed_oversized_revision: None,
                    token: Some(token),
                    schedule,
                    in_flight: Some(attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.recovery_checkpoint_worker_active = true;
        });

        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(timing.durable_complete_by, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert_eq!(state.in_flight.as_ref(), Some(&attempt));
            assert!(state.deadline_reported);
            assert!(state.protection_warning);
            assert!(workspace.recovery_warning.is_some());
            assert!(workspace._recovery_timer.is_none());
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
            workspace._recovery_timer = None;
        });

        let latest = "latest after overdue 中文 \u{1f680}\n";
        replace_document(&workspace, 0, latest, cx);
        workspace.update(cx, |workspace, _| {
            workspace._recovery_timer = None;
        });
        cx.background_executor.advance_clock(Duration::from_secs(1));
        let retry_at = cx.background_executor.now();
        workspace.update(cx, |workspace, cx| {
            workspace._recovery_timer = None;
            workspace.checkpoint_recovery_at(retry_at, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let retry = workspace
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
            assert!(workspace.recovery_warning.is_some());
        });
        cx.run_until_parked();

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, latest);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_warning.is_none());
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

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
            assert!(!workspace.recovery_checkpoint_worker_active);
            assert!(workspace.recovery_warning.is_some());
            assert!(workspace._recovery_timer.is_none());
        });

        let smaller = "small recoverable edit 中文 \u{1f680}\n";
        replace_document(&workspace, 0, smaller, cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        assert_eq!(store.checkpoint_batch_count_for_test(), 1);
        assert_eq!(protector.calls(), 1);
        assert_eq!(store.recover().unwrap().records[0].record.text, smaller);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_warning.is_none());
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

        replace_document(&workspace, 0, "below the plaintext ceiling\n", cx);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 1);
        assert_eq!(store.checkpoint_batch_count_for_test(), 1);
        assert!(workspace.read_with(cx, |workspace, _| workspace.recovery_warning.is_some()));

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

    #[gpui::test]
    fn transient_protection_failure_retries_the_same_revision_and_succeeds(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transient-protection.md");
        fs::write(&path, "disk\n").unwrap();
        let protector = Arc::new(CountingRecoveryProtector::new(CountingProtection::FailOnce));
        let store =
            RecoveryStore::new_at(dir.path().join("recovery-store"), protector.clone()).unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

        let text = "retry this exact revision 中文 \u{1f680}\n";
        replace_document(&workspace, 0, text, cx);
        let revision = workspace.read_with(cx, |workspace, app| {
            workspace.document_at(0).unwrap().read(app).revision()
        });
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(protector.calls(), 1);
        assert!(workspace.read_with(cx, |workspace, _| workspace.recovery_warning.is_some()));

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
            assert!(workspace.recovery_warning.is_none());
        });
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        replace_document(&workspace, 0, "current revision\n", cx);

        let now = cx.background_executor.now();
        let (id, key, revision, token) = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
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
        let attempt = RecoveryAttempt {
            token: token.clone(),
            revision: revision.saturating_sub(1),
            timing,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        workspace.update(cx, |workspace, cx| {
            workspace.recovery = Some(store);
            workspace.recovery_schedules.insert(
                id,
                DocumentRecoveryState {
                    key,
                    revision,
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(workspace.recovery_warning.is_some());
        });
    }

    #[gpui::test]
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
            metadata: RecoveryMetadata::from_loaded_file(&crate::fs::load(&incoming_path).unwrap()),
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

        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let edited = "edited during eviction reservation\n";
        replace_document(&workspace, 0, edited, cx);
        let immediate_warning =
            workspace.read_with(cx, |workspace, _| workspace.recovery_warning.clone());

        release.send(()).unwrap();
        assert!(matches!(
            worker.join().unwrap(),
            CheckpointOutcome::Written(_)
        ));
        let warning = "Recovery protection is unavailable for at least one dirty document. Editing and source files are unchanged.";
        assert_eq!(immediate_warning.as_deref(), Some(warning));
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.recovery_warning.as_deref(), Some(warning));
        });

        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery_warning.is_none());
            assert!(
                workspace
                    .recovery_schedules
                    .values()
                    .all(|state| !state.protection_warning)
            );
        });
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, edited);
    }

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
    fn recovery_idle_deadline_restores_exact_cjk_and_emoji_text(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        let (workspace, cx) = open_test_workspace(cx, path);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });

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

        let (restored_workspace, cx) = open_test_workspace_with(cx, None);
        restored_workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
    fn no_record_startup_recovery_clears_pending_state(cx: &mut TestAppContext) {
        let (workspace, cx) =
            open_test_workspace_with_startup_recovery(cx, None, StartupRecovery::default);
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.startup_recovery_pending);
            assert!(workspace.recovery.is_none());
            assert!(workspace.tabs.is_empty());
        });
    }

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
        });

        let edited_at = cx.background_executor.now();
        replace_document(&workspace, 0, "typed while recovery opens\n", cx);
        let id = workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace
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
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert!(state.protection_warning);
            assert!(workspace.recovery_warning.is_some());
        });

        let store_ready_at = edited_at + Duration::from_secs(3);
        complete_startup_with_store(&workspace, store, cx);
        workspace.read_with(cx, |workspace, _| {
            let state = workspace.recovery_schedules.get(&id).unwrap();
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
                workspace._recovery_timer.is_some(),
                "the overdue schedule must be re-armed when recovery becomes available"
            );
        });
        workspace.update(cx, |workspace, cx| {
            workspace.checkpoint_recovery_at(store_ready_at, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            let attempt = workspace
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

    #[gpui::test]
    fn populated_startup_recovery_does_not_block_initial_file_or_early_edits(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("initial.md");
        fs::write(&path, "disk\n").unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        write_recovery_checkpoint(&store, &path, "older recovered text\n");
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
            assert_eq!(workspace.tabs.len(), 1);
            let document = workspace.document_at(0).unwrap().read(app);
            assert!(document.is_dirty());
            assert_eq!(document.text(app), latest);
            document.id()
        });
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.recovery.is_some());
            assert!(workspace.recovery_schedules.contains_key(&id));
            assert_eq!(
                workspace.status.as_deref(),
                Some(
                    "Restored 0 recovery checkpoint(s); skipped 1 unavailable or invalid record(s)."
                )
            );
        });
    }

    #[gpui::test]
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
            assert!(!workspace.startup_recovery_pending);
            let state = workspace.recovery_schedules.get(&id).unwrap();
            assert_eq!(
                state.schedule.next_deadline(),
                Some(restored_at + Duration::from_secs(10))
            );
            assert!(state.in_flight.is_none());
            assert!(workspace._recovery_timer.is_some());
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

    #[gpui::test]
    fn restored_dirty_checkpoint_refreshes_at_ten_seconds_from_its_durable_baseline(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored-refresh.md");
        fs::write(&path, "disk\n").unwrap();
        let loaded = crate::fs::load(&path).unwrap();
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
                },
                source_conflicted: false,
            }],
            issues: Vec::new(),
        };
        let (workspace, cx) = open_test_workspace_with(cx, None);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
        let restored_at = cx.background_executor.now();

        let restored = cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                restore_recovery_for_test(workspace, scan, window, cx)
            })
        });
        assert_eq!(restored, (1, 0));
        workspace.read_with(cx, |workspace, app| {
            let document = workspace.document_at(0).unwrap().read(app);
            let state = workspace.recovery_schedules.get(&document.id()).unwrap();
            assert_eq!(
                state.schedule.next_deadline(),
                Some(restored_at + Duration::from_secs(10))
            );
            assert!(workspace._recovery_timer.is_some());
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

    #[gpui::test]
    fn changed_initial_source_restores_recovery_as_conflicted(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changed-source.md");
        fs::write(&path, "original disk\n").unwrap();
        let loaded = crate::fs::load(&path).unwrap();
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
            workspace.recovery = None;
            workspace.startup_recovery_pending = true;
            workspace.invalidate_recovery(&key, None, cx);
            assert!(workspace.pending_recovery_retirements.contains_key(&key));
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
            assert!(workspace.pending_recovery_retirements.is_empty());
        });
    }

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
    fn stale_written_checkpoint_does_not_signal_current_durability() {
        let written = CheckpointBatchOutcome::Written;
        assert!(current_checkpoint_write_completed(true, 7, 7, &written));
        assert!(!current_checkpoint_write_completed(true, 8, 7, &written));
        assert!(!current_checkpoint_write_completed(false, 7, 7, &written));
        assert!(!current_checkpoint_write_completed(
            true,
            7,
            7,
            &CheckpointBatchOutcome::Deferred,
        ));
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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
                revision: first_revision,
                timing: first_schedule.checkpoint_dispatched(now).unwrap(),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            let second_attempt = RecoveryAttempt {
                token: second_token.clone(),
                revision: second_revision,
                timing: second_schedule.checkpoint_dispatched(now).unwrap(),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            workspace.recovery_schedules.insert(
                first_id,
                DocumentRecoveryState {
                    key: first_key,
                    revision: first_revision,
                    suppressed_oversized_revision: None,
                    token: Some(first_token),
                    schedule: first_schedule,
                    in_flight: Some(first_attempt.clone()),
                    deadline_reported: false,
                    protection_warning: false,
                },
            );
            workspace.recovery_schedules.insert(
                second_id,
                DocumentRecoveryState {
                    key: second_key,
                    revision: second_revision,
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
    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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
                workspace.recovery_retirement_batches.get(&first_key),
                Some(&old_batch)
            );
            assert!(
                !workspace
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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first.clone());
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
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

    #[gpui::test]
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
        let (workspace, cx) = open_test_workspace(cx, first);
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store.clone());
        });
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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

    #[gpui::test]
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
    #[gpui::test]
    fn recovered_document_retains_text_metadata_and_conflict_state(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.txt");
        fs::write(&path, b"\xFF\xFEh\x00i\x00\r\x00\n\x00").unwrap();
        let loaded = crate::fs::load(&path).unwrap();
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
                },
                source_conflicted: true,
            }],
            issues: Vec::new(),
        };
        let (workspace, cx) = open_test_workspace_with(cx, None);
        let store = RecoveryStore::new_at(
            dir.path().join("recovery-store"),
            Arc::new(TestRecoveryProtector),
        )
        .unwrap();
        workspace.update(cx, |workspace, _| {
            workspace.recovery = Some(store);
        });

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
            assert!(workspace.recovery_schedules.contains_key(&id));
            assert!(workspace._recovery_timer.is_some());
        });
        document.read_with(cx, |document, app| {
            assert!(document.is_dirty());
            assert!(document.is_externally_changed());
            assert_eq!(document.text(app), recovered_text);
            let checkpoint = document.recovery_checkpoint(app);
            assert_eq!(checkpoint.metadata.encoding_name, "UTF-16LE");
            assert!(checkpoint.metadata.had_bom);
            assert_eq!(checkpoint.metadata.newline, crate::fs::Newline::Crlf);
            assert_eq!(checkpoint.metadata.original_stamp, original_stamp);
        });
    }

    #[test]
    fn workspace_panel_widths_preserve_preferences_and_the_document_floor() {
        let widths = resolved_workspace_panel_widths(
            gpui::px(224.),
            gpui::px(288.),
            true,
            true,
            gpui::px(1200.),
        );

        assert_eq!(widths.left, gpui::px(224.));
        assert_eq!(widths.right, gpui::px(288.));

        let collapsed = resolved_workspace_panel_widths(
            gpui::px(224.),
            gpui::px(288.),
            false,
            false,
            gpui::px(1200.),
        );
        assert_eq!(collapsed.left, gpui::px(0.));
        assert_eq!(collapsed.right, gpui::px(0.));

        let restored = resolved_workspace_panel_widths(
            gpui::px(224.),
            gpui::px(288.),
            true,
            true,
            gpui::px(1200.),
        );
        assert_eq!(restored.left, gpui::px(224.));
        assert_eq!(restored.right, gpui::px(288.));

        let narrowed = resolved_workspace_panel_widths(
            gpui::px(640.),
            gpui::px(720.),
            true,
            true,
            gpui::px(720.),
        );
        assert_eq!(
            narrowed.left + narrowed.right,
            gpui::px(440.),
            "restoring both panels must leave the document its 280px \
             useful-width floor"
        );
        assert_eq!(narrowed.left, gpui::px(crate::metrics::SIDE_PANEL.min));
        assert_eq!(narrowed.right, gpui::px(crate::metrics::RIGHT_PANEL.min));

        let left_only = resolved_workspace_panel_widths(
            gpui::px(640.),
            gpui::px(288.),
            true,
            false,
            gpui::px(720.),
        );
        assert_eq!(left_only.left, gpui::px(440.));

        for (viewport, expected_side_budget) in [(600., 320.), (300., 20.)] {
            let forced = resolved_workspace_panel_widths(
                gpui::px(640.),
                gpui::px(720.),
                true,
                true,
                gpui::px(viewport),
            );
            assert_eq!(
                forced.left + forced.right,
                gpui::px(expected_side_budget),
                "forced viewport {viewport}px must preserve the document budget"
            );
        }

        let tiny_right = resolved_workspace_panel_widths(
            gpui::px(224.),
            gpui::px(720.),
            false,
            true,
            gpui::px(300.),
        );
        assert_eq!(tiny_right.right, gpui::px(20.));
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
                gpui::px(900.),
                gpui::px(288.),
                true,
                gpui::px(1200.),
            ),
            gpui::px(632.),
            "the opposite panel and document floor bound the dragged side"
        );
        assert_eq!(
            clamped_dragged_panel_width(
                WorkspaceResizeEdge::Right,
                gpui::px(40.),
                gpui::px(224.),
                true,
                gpui::px(1200.),
            ),
            gpui::px(crate::metrics::RIGHT_PANEL.min),
            "normal viewports retain the panel's useful minimum"
        );
        assert_eq!(
            clamped_dragged_panel_width(
                WorkspaceResizeEdge::Right,
                gpui::px(720.),
                gpui::px(180.),
                true,
                gpui::px(300.),
            ),
            gpui::px(0.),
            "a forced tiny viewport yields to the document rather than panicking"
        );
    }

    #[gpui::test]
    fn dragging_the_workspace_divider_updates_the_owned_column(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
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
                gpui_component::Root::new(workspace, window, cx)
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

    #[gpui::test]
    fn keyboard_resizes_the_focused_workspace_divider(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
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
                gpui_component::Root::new(workspace, window, cx)
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

    /// Windows UI Automation maps Splitter to a read-only RangeValue provider
    /// in the pinned accesskit_consumer, so the equivalent Slider role is a
    /// compatibility contract rather than a presentational choice.
    #[test]
    fn windows_exposes_panel_resize_as_a_writable_range_control() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let resize = source
            .split_once("fn render_panel_resize_handle")
            .expect("render_panel_resize_handle")
            .1;
        let resize = resize
            .split("\n    fn render_title_commands")
            .next()
            .unwrap_or(resize);

        assert!(resize.contains("if cfg!(target_os = \"windows\")"));
        assert!(resize.contains("gpui::Role::Slider"));
        assert!(resize.contains("gpui::accesskit::Orientation::Horizontal"));
        assert!(resize.contains("gpui::Role::Splitter"));
        assert!(resize.contains("gpui::accesskit::Orientation::Vertical"));
        assert!(resize.contains("gpui::accesskit::Action::SetValue"));
        assert!(resize.contains("gpui::accesskit::ActionData::NumericValue(value)"));
    }

    #[test]
    fn short_names_are_left_alone() {
        for name in ["a.md", "README.md", "architecture.md"] {
            assert_eq!(elide_tab_label(name), name);
        }
    }

    /// Tab overlays belong to the tab's own hitbox, never a prefix wrapper.
    #[test]
    fn tab_affordances_cover_the_tab() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source.find("fn render_tabs").expect("render_tabs");
        let body = &source[start..];
        let end = body
            .find("\n    fn render_web_path_controls")
            .unwrap_or(body.len());
        let body = &body[..end];
        let gate = body
            .find(".when(!web_active && path.is_some(), |tab|")
            .expect("the file-only Web-active overlay gate");
        let end = body[gate..]
            .find("// A preview tab")
            .map(|end| gate + end)
            .unwrap_or(body.len());
        let affordance = &body[gate..end];

        assert!(affordance.contains(".tooltip(") && affordance.contains(".context_menu("));
        assert!(
            affordance.contains("path.is_some()"),
            "memory documents have no filesystem path, so they must not offer file-path controls"
        );
        assert!(affordance.contains("tab.child("));
        assert!(affordance.contains(".absolute()") && affordance.contains(".inset_0()"));
        assert!(!body.contains(".prefix("));
        assert!(body.contains(".aria_label(aria_label)"));
        assert!(
            body.contains(".when(!web_active, |this|") && body.contains("UnsavedChanges"),
            "the dirty marker tooltip needs its own Web-active gate"
        );
        assert_eq!(
            super::TAB_CLOSE_ACCESSIBILITY_ID,
            "markturbo-document-tab-close"
        );
        assert!(body.contains("self.tabs.len() == 1 && self.tabs.active_index() == ix"));
        assert_eq!(
            body.matches("accessibility_id(TAB_CLOSE_ACCESSIBILITY_ID)")
                .count(),
            2,
            "both the dirty marker and clean close button need the same active-document UIA id"
        );
        assert!(body.contains(".role(gpui::Role::Button)"));
    }

    #[test]
    fn web_mode_keeps_fixed_chrome_and_side_panels() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let title = source
            .find("fn render_title_commands")
            .expect("title chrome renderers");
        let title_body = &source[title..];
        let title_end = title_body
            .find("\n    fn render_status_bar")
            .unwrap_or(title_body.len());
        let title_body = &title_body[..title_end];
        let path = source
            .find("fn render_web_path_controls")
            .expect("fixed Web commands");
        let path_end = source[path..]
            .find("\n    fn render_panel_resize_handle")
            .map(|end| path + end)
            .unwrap_or(title);
        let path_body = &source[path..path_end];
        let status = source
            .find("fn render_status_bar")
            .expect("render_status_bar");
        let status_body = &source[status..];
        let status_end = status_body
            .find("\n}\n\nfn empty_hint")
            .unwrap_or(status_body.len());
        let status_body = &status_body[..status_end];
        let render = source
            .find("impl Render for Workspace")
            .map(|start| &source[start..])
            .expect("workspace render");

        assert!(!title_body.contains("let navigator = if web_active"));
        assert!(title_body.contains("self.render_navigator(!web_active, cx).into_any_element()"));
        assert!(!path_body.contains(".tooltip(") && !path_body.contains(".context_menu("));
        assert!(title_body.contains("render_left_toggle(tooltips, cx)"));
        assert!(title_body.contains("render_right_toggle(tooltips, cx)"));
        assert!(!title_body.contains(".when(!self.left_panel_open"));
        assert!(!title_body.contains(".when(!self.right_panel_open"));
        assert!(!title_body.contains("web_active && !self.right_panel_open"));
        assert!(title_body.contains("IconName::FolderOpen"));
        assert!(title_body.contains("IconName::Globe"));
        assert!(!title_body.contains(".label(i18n::t(i18n::Key::OpenFolder"));
        assert!(!title_body.contains(".label(i18n::t(i18n::Key::Translate"));
        for key in ["OpenFolderPicker", "Translate", "Settings"] {
            assert!(
                title_body.contains(&format!("i18n::t(i18n::Key::{key}, cx)")),
                "the {key} icon-only command must pass its localized name to \
                 the shared accessible chrome button"
            );
        }
        assert!(
            !title_body.contains(".xsmall()"),
            "title-bar commands must keep the repository's 24px pointer target"
        );
        assert!(status_body.contains(".when(web_active, |button|"));
        assert!(status_body.contains("button.label(auto_refresh_label)"));
        assert!(status_body.contains(".when(!web_active, |button| button.tooltip("));
        assert!(render.contains("let right_panel = (!self.show_welcome)"));
        let side_panel = &render[render.find("let side_panel").expect("side panel")..];
        let side_panel = &side_panel[..side_panel
            .find("let panel_widths")
            .unwrap_or(side_panel.len())];
        assert!(side_panel.contains("left_panel_visible") && side_panel.contains(".then("));
        assert!(!side_panel.contains("web_active"));
        assert!(
            render.contains(".child(self.render_status_bar(cx))"),
            "the fixed status bar remains below the child HWND for command feedback"
        );
    }

    #[test]
    fn web_mode_keeps_side_panel_actions_available() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        for (signature, next) in [
            ("fn on_toggle_left_panel", "fn on_toggle_right_panel"),
            ("fn on_toggle_right_panel", "/// Open files dropped"),
            ("fn on_focus_search", "/// The documents the current search"),
        ] {
            let start = source
                .find(signature)
                .unwrap_or_else(|| panic!("{signature}"));
            let body = &source[start..];
            let end = body.find(next).unwrap_or(body.len());
            let body = &body[..end];
            assert!(!body.contains("if self.web_active(cx)"));
        }
    }

    #[test]
    fn unrelated_tree_changes_do_not_rescan_harness() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source.find("fn drain_watcher").expect("drain_watcher");
        let body = &source[start..];
        let end = body.find("// --- Actions").unwrap_or(body.len());
        let body = &body[..end];

        assert!(!body.contains("tree_changed || harness_changed"));
        assert!(body.contains("change.path().starts_with(watcher_root)"));
        assert!(body.contains("harness.has_artifact_under(change.path())"));
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

    /// A preview tab must be replaced, not accumulated.
    ///
    /// Source-level: the behavior needs a window and a real click sequence, but
    /// what makes it work is that `open_file_as` takes the outgoing preview and
    /// closes it. Someone simplifying that away gets forty tabs back.
    ///
    /// The *slot's* own rules — closing the preview tab empties it, closing a
    /// different one leaves it — are plain data and are tested for real in
    /// [`crate::views::tabs`].
    #[test]
    fn opening_a_preview_replaces_the_previous_one() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("pub fn open_file_as")
            .expect("open_file_as must exist");
        let body = &source[start..];
        let end = body.find("\n    /// Focus the tab").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("take_preview()"),
            "the outgoing preview must be taken, or previews accumulate"
        );
        assert!(
            body.contains("close_tab"),
            "and closed, or the slot is only forgotten rather than freed"
        );
        assert!(
            body.contains("is_dirty"),
            "a preview with unsaved edits must be kept, not silently discarded"
        );
    }

    /// Per-document subscriptions must die with their tab.
    ///
    /// Source-level because the leak is invisible to any assertion: the
    /// subscriptions stayed alive and simply fired against a document nobody
    /// could see. What prevents it is that they are *owned by the tab* rather
    /// than pushed onto a workspace-wide `Vec` that only ever grows.
    #[test]
    fn a_closed_tab_takes_its_subscriptions_with_it() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn open_file_inner")
            .expect("open_file_inner must exist");
        let body = &source[start..];
        let end = body.find("\n    fn close_tab").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            !body.contains("_subscriptions.push"),
            "a per-document subscription must not go into the workspace-wide \
             list: that list is never pruned, so closing the tab left two \
             subscriptions to a released document alive for the session"
        );
        assert!(
            body.contains("DocumentTab {"),
            "they belong to the tab, which drops them when it closes"
        );
    }

    /// A status message must not be cleared by the previous message's timer.
    ///
    /// Source-level: the failure needs six seconds of wall clock and two
    /// messages, which is not a test worth having. What prevents it is the
    /// generation check plus a single timer slot — a detached task per message
    /// meant every one of them cleared the bar unconditionally when it fired,
    /// so the second message vanished on the first one's schedule.
    #[test]
    fn a_status_message_outlives_the_one_before_it() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source.find("fn set_status").expect("set_status must exist");
        let body = &source[start..];
        let end = body
            .find("\n    fn remove_recovery_schedule")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            !body.contains(".detach()"),
            "the timer must be held in a slot, so a new message cancels the \
             old one's timer rather than leaving it to fire"
        );
        assert!(
            body.contains("status_generation == generation"),
            "and the timer must check it is still clearing its own message"
        );
    }

    ///
    /// Source-level: a real drop needs a window and an OS drag. What makes the
    /// feature work rather than appear to is that a directory becomes the
    /// workspace, a document opens, and anything else is *reported* — a drop
    /// that silently does nothing reads as a broken window.
    #[test]
    fn dropping_paths_handles_all_three_cases() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn on_drop_paths")
            .expect("on_drop_paths must exist");
        let body = &source[start..];
        let end = body
            .find("\n    /// The path of whichever tab")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("is_dir()"),
            "a folder must open as a workspace"
        );
        assert!(
            body.contains("is_openable("),
            "a dropped file must go through the same gate the file tree uses. \
             `DocType::of(..).is_document()` is not that gate: it judges the \
             extension alone, so a NUL-filled `.log` passes, is decoded into \
             the editor's `String`, and the first Ctrl+S re-encodes from that \
             `String` — dropping every byte the decoder could not map. The \
             conflict check in `fs::save` guards a different failure: nothing \
             changed on disk, so that write is authorized"
        );
        assert!(
            body.contains("set_status"),
            "an unopenable drop must say so rather than doing nothing visible"
        );
        assert!(
            source.contains("fn open_file_target") && source.contains("self.root.is_none()"),
            "a bare file drop should adopt its parent as the workspace"
        );
    }

    /// The title bar must leave somewhere to grab the window.
    ///
    /// Source-level because reproducing it needs a real `WM_NCHITTEST` against
    /// a laid-out bar. What broke: the tab strip was wrapped in a `flex_1` div
    /// carrying `stop_propagation`, so the wrapper covered the whole bar and
    /// killed *both* drag paths — GPUI's, because `TitleBar`'s own
    /// `on_mouse_down` never saw a stopped event, and Windows', because
    /// `handle_nc_mouse_down_msg` returns `Some(0)` for a handled press and
    /// `DefWindowProc` never receives the `HTCAPTION`. The window simply stopped
    /// moving, with nothing in the log.
    #[test]
    fn the_title_bar_keeps_a_drag_handle() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn render_title_commands")
            .expect("title chrome renderers must exist");
        let body = &source[start..];
        let end = body
            .find("\n    fn render_status_bar")
            .unwrap_or(body.len());
        let body = &body[..end];

        // Every press-claiming region must be sized to its own content. The
        // count is not the invariant — the bar legitimately grew a third claim
        // when the navigator arrived — what matters is that no claim sits on a
        // box that grows to fill the bar, because that box is the drag handle.
        //
        // Checked by walking backwards from each handler to the `h_flex()` or
        // `div()` that opens its element, rather than by matching indentation:
        // `cargo fmt` rewrites whitespace, and an assertion pinned to it
        // silently becomes an assertion about nothing.
        let claims: Vec<usize> = body
            .match_indices("cx.stop_propagation()")
            .map(|(at, _)| at)
            .collect();
        assert!(
            claims.len() >= 2,
            "the buttons must claim their presses back, or they do nothing \
             inside a WindowControlArea::Drag region"
        );
        for at in claims {
            let opener = body[..at]
                .rfind("h_flex()")
                .into_iter()
                .chain(body[..at].rfind("div()"))
                .max()
                .expect("every handler is on some element");
            let element = &body[opener..at];
            assert!(
                !element.contains(".flex_1()"),
                "a press-claiming region sits on a flex_1 box, which covers the \
                 whole bar and kills both drag paths: GPUI's, because \
                 `TitleBar`'s own `on_mouse_down` never sees a stopped event, \
                 and Windows', because `handle_nc_mouse_down_msg` returns \
                 `Some(0)` for a handled press so `DefWindowProc` never gets \
                 the `HTCAPTION`.\nOffending element: {element}"
            );
        }

        assert!(
            body.contains("div().flex_1().min_w_0().h_full()"),
            "the bar needs a handler-free filler, or every pixel beside the \
             tabs is claimed and the window cannot be dragged"
        );

        // And the tab strip itself must shrink to its content rather than
        // claiming the width.
        let start = source.find("fn render_tabs").expect("render_tabs");
        let tabs = &source[start..];
        let end = tabs.find("\n    fn render_welcome").unwrap_or(tabs.len());
        let tabs = &tabs[..end];
        assert!(
            !tabs.contains(".w_full()"),
            "a full-width tab strip leaves no slack in the title bar to drag"
        );
    }

    /// The title and body rows resolve their tracks from one owned width model,
    /// not from a previous frame's measured layout.
    ///
    /// Source-level because the regression is architectural: two sibling trees
    /// can agree in a static window and still diverge after maximize/restore.
    /// The Windows reproduction measured both edges at x=371 on launch; after
    /// one restore the title edge stayed at x=371 while the body edge moved to
    /// x=409. Explicitly assigning the same width to both rows makes that state
    /// impossible while preserving row-major accessibility order.
    #[test]
    fn workspace_rows_share_one_owned_width_model() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let region = source
            .split_once("fn workspace_region")
            .map(|(_, body)| body)
            .expect("workspace_region must own shared row geometry");
        let region = region.split("\nfn ").next().unwrap_or(region);
        assert!(region.contains(".child(content)"));
        assert!(region.contains("this.w(width).flex_none()"));

        let render = source
            .split_once("impl Render for Workspace")
            .map(|(_, body)| body)
            .expect("the Workspace Render impl");
        let render = render.split("\n/// Keybindings").next().unwrap_or(render);
        for (id, width) in [
            ("left-title-region", "Some(panel_widths.left)"),
            ("document-title-region", "None"),
            ("right-title-region", "Some(panel_widths.right)"),
            ("left-workspace-column", "Some(panel_widths.left)"),
            ("document-workspace-column", "None"),
            ("right-workspace-column", "Some(panel_widths.right)"),
        ] {
            let id_at = render
                .find(&format!("\"{id}\""))
                .unwrap_or_else(|| panic!("missing {id}"));
            let call_at = render[..id_at]
                .rfind("workspace_region(")
                .unwrap_or_else(|| panic!("{id} is not built by workspace_region"));
            let call = &render[call_at..(id_at + 100).min(render.len())];
            assert!(
                id_at - call_at < 80 && call.contains(width),
                "{id} must use the shared {width} track"
            );
        }
        assert!(
            !render.contains("h_resizable(\"workspace-split\")"),
            "the outer Workspace split cannot remain a second geometry owner"
        );
    }

    /// Platform title-bar behavior sits behind a title row whose tracks share
    /// the body row's resolved widths.
    ///
    /// Source-level: this is pure layout, invisible to any runtime assertion.
    /// What it guards is the arrangement itself — a title bar drawn across the
    /// panels makes them read as content parked underneath it, where this reads
    /// as the main view extending sideways, which is the whole point.
    #[test]
    fn platform_title_bar_sits_behind_the_owned_title_row() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Render impl")
            .1;
        let body = render.split("\n/// Keybindings").next().unwrap_or(render);

        assert!(
            body.contains("let title_regions = h_flex()")
                && body.contains("let body_regions = h_flex()"),
            "title and body need explicit rows over the same three tracks"
        );
        assert!(
            body.contains("let title_bar_backdrop = self.render_title_bar_backdrop(cx)"),
            "the platform TitleBar must remain present for native controls and dragging"
        );
        assert!(
            body.contains("Some(panel_widths.left)") && body.contains("Some(panel_widths.right)"),
            "the user-owned widths must size the complete left and right columns"
        );
        assert!(
            body.contains("left_resize_handle")
                && body.contains("WorkspaceResizeEdge::Left")
                && body.contains("WorkspaceResizeEdge::Right"),
            "both panel boundaries need one visible, draggable owner"
        );
        let title = body
            .split_once("let workspace_title = div()")
            .expect("the workspace title row")
            .1;
        let title = title.split("let body_regions").next().unwrap_or(title);
        let backdrop = title
            .find(".child(title_bar_backdrop)")
            .expect("the platform title-bar backdrop");
        let regions = title
            .find(".child(title_regions)")
            .expect("the owned title regions");
        assert!(
            backdrop < regions,
            "application chrome must paint over the platform backdrop while leaving \
             its native controls and drag regions active"
        );
        let backdrop_renderer = source
            .split_once("fn render_title_bar_backdrop")
            .expect("render_title_bar_backdrop")
            .1;
        let backdrop_renderer = backdrop_renderer
            .split("\n    fn render_left_title_bar")
            .next()
            .unwrap_or(backdrop_renderer);
        assert!(
            backdrop_renderer.contains("TitleBar::new()")
                && backdrop_renderer.contains(".border_b_0()"),
            "the active document tab cannot connect to its content while the \
             platform backdrop paints another full-width bottom rule"
        );
        assert!(
            !source.contains("workspace_split_state")
                && !source.contains("details_split_state")
                && !source.contains("left_chrome_width"),
            "no second geometry cache may reconstruct the shared column boundaries"
        );
        assert!(
            body.contains("native_window_controls_width(window)")
                && body.contains("render_right_title_bar(controls_width, cx)"),
            "application commands must stay immediately before native controls"
        );
        assert!(
            body.contains("self.layout_width.unwrap_or(window.viewport_size().width)")
                && body.contains("this.layout_width = Some(width)"),
            "panel budgeting must use the width Root actually assigned, including Linux CSD"
        );

        assert!(
            body.contains("left_panel_visible.then(||") && body.contains("left-title-region"),
            "the left panel must be omittable, not merely narrow"
        );
        assert!(
            body.contains("right_panel_visible.then(||") && body.contains("right-title-region"),
            "the right panel must be omittable"
        );
    }

    /// Panel toggles stay in one global overlay instead of moving between
    /// columns as panels open and close.
    ///
    /// Source-level: pure layout, invisible to any runtime assertion. Two things
    /// are guarded. The sides — put together, a control's position contradicts
    /// what it opens, and the user reads the icon instead of recognizing the
    /// side. And the handoff — a toggle rendered in both places at once is two
    /// buttons with the same element id, and one rendered in neither leaves an
    /// open panel with no way to close it.
    #[test]
    fn panel_toggles_and_global_commands_stay_in_the_title_bar() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let commands = source
            .split_once("fn render_title_commands")
            .expect("render_title_commands")
            .1;
        let commands = commands
            .split("\n    /// Platform title-bar behavior")
            .next()
            .unwrap_or(commands);
        let open = commands
            .find("\"open-folder\",")
            .expect("the open-folder command");
        let translate = commands
            .find("\"translate\",")
            .expect("the translate command");
        let right = commands
            .find("render_right_toggle")
            .expect("the right panel toggle");
        let settings = commands.find("\"settings\",").expect("the settings button");
        assert!(
            open < translate && translate < right && right < settings,
            "global commands need one stable order before native window controls"
        );

        let document = source
            .split_once("fn render_document_title_controls")
            .expect("render_document_title_controls")
            .1;
        let document = document
            .split("\n    fn render_right_title_bar")
            .next()
            .unwrap_or(document);
        let left_reservation = document
            .find("metrics::title_bar_leading_inset()")
            .expect("space reserved for the fixed left toggle");
        let navigator = document
            .find(".child(navigator)")
            .expect("the document navigator");
        let tabs = document
            .find("self.render_tabs(cx)")
            .expect("the tab strip");
        assert!(left_reservation < navigator && navigator < tabs);

        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Workspace Render impl")
            .1;
        assert!(render.contains("!self.left_panel_open"));
        assert!(render.contains("side_panel") && render.contains("left_title_bar"));
        assert!(render.contains("render_left_toggle_overlay(!web_active, cx)"));
        assert!(render.contains("render_title_commands_overlay("));
        assert_eq!(source.matches("self.render_left_toggle(").count(), 1);
        assert_eq!(source.matches("self.render_title_commands(").count(), 1);
        for (renderer, toggle) in [
            ("fn render_side_panel", "render_left_toggle"),
            ("fn render_right_panel", "render_right_toggle"),
        ] {
            let start = source
                .find(renderer)
                .unwrap_or_else(|| panic!("{renderer}"));
            let body = &source[start..];
            let end = body.find("\n    /// ").unwrap_or(body.len());
            assert!(
                !body[..end].contains(toggle),
                "{renderer} must not duplicate the global `{toggle}` control"
            );
        }
    }

    #[test]
    fn workspace_structure_follows_the_visual_and_accessibility_order() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        assert!(
            !source.contains("TAB_GROUP_") && !source.contains(".tab_group()"),
            "visual structure should establish keyboard order without a second ordering model"
        );

        let region = source
            .split_once("fn workspace_region")
            .expect("workspace_region must own row geometry")
            .1;
        let region = region.split("\nfn ").next().unwrap_or(region);
        assert!(
            region.contains(".when_some(width, |this, width| this.w(width).flex_none())")
                && region.contains(".when(width.is_none(), |this| this.flex_1())"),
            "both workspace rows must resolve their tracks through one helper"
        );

        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Workspace Render impl")
            .1;
        let render = render.split("\n/// Keybindings").next().unwrap_or(render);
        for region in [
            "left-title-region",
            "document-title-region",
            "right-title-region",
            "left-workspace-column",
            "document-workspace-column",
            "right-workspace-column",
        ] {
            assert!(
                render.contains(&format!("\"{region}\"")),
                "missing shared workspace region {region}"
            );
        }

        let frame = render
            .split_once("let workspace_frame = v_flex()")
            .expect("a row-major workspace frame")
            .1;
        let title = frame
            .find(".child(workspace_title)")
            .expect("the title row");
        let body = frame.find(".child(workspace_body)").expect("the body row");
        assert!(
            title < body,
            "title controls must precede body content in paint, Tab, and AccessKit order"
        );

        let title = render
            .split_once("let workspace_title = div()")
            .expect("the stable title layer")
            .1;
        let title = title.split("let workspace_body").next().unwrap_or(title);
        let backdrop = title
            .find(".child(title_bar_backdrop)")
            .expect("the platform title bar");
        let backgrounds = title
            .find(".child(title_regions)")
            .expect("the title backgrounds");
        let leading = title
            .find(".child(left_toggle_overlay)")
            .expect("the leading title control");
        let document = title
            .find(".child(document_title_controls)")
            .expect("the document title controls");
        let trailing = title
            .find(".child(title_commands_overlay)")
            .expect("the trailing title controls");
        assert!(
            backdrop < backgrounds
                && backgrounds < leading
                && leading < document
                && document < trailing,
            "title controls need one stable left-to-right semantic and paint order"
        );
    }

    #[test]
    fn the_left_panel_uses_vertical_sidebar_navigation() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn render_side_panel")
            .expect("render_side_panel");
        let body = &source[start..];
        let end = body
            .find("\n    /// Document outline")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(body.contains("SidebarNavigationButton::new("));
        assert!(body.contains("i18n::t(panel.label(), cx)"));
        assert!(body.contains(".selected(panel == self.side_panel)"));
        assert!(body.contains("panel.icon()"));
        assert!(
            body.contains(".py_2()") && !body.contains(".p_2()"),
            "the navigation row owns the same horizontal inset as root file rows"
        );
        assert!(!body.contains("TabBar::new(\"side-panel\")"));

        let component = source
            .split_once("impl RenderOnce for SidebarNavigationButton")
            .expect("SidebarNavigationButton renderer")
            .1;
        let component = component
            .split("\nfn path_affects_harness")
            .next()
            .unwrap_or(component);
        assert!(component.contains("BaseToggle::new(self.id)"));
        assert!(component.contains(".accessibility_label(self.label)"));
        assert!(component.contains(".justify_start()"));
        assert!(component.contains(".px(metrics::row_pad())"));
    }

    #[test]
    fn harness_updates_rebuild_the_details_panel_and_web_bounds() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source.find("pub fn open_folder").expect("open_folder");
        let body = &source[start..];
        let end = body
            .find("\n    /// Open `path` as a document")
            .unwrap_or(body.len());
        let body = &body[..end];

        let observer = body.find("cx.observe(&harness").expect("Harness observer");
        let observer = &body[observer..];
        assert!(observer.contains("this.web_dirty(cx)"));
        assert!(observer.contains("cx.notify()"));
    }

    #[test]
    fn details_panel_is_always_available_and_includes_document_details() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn render_right_panel")
            .expect("render_right_panel");
        let body = &source[start..];
        let end = body.find("\n    /// The left panel").unwrap_or(body.len());
        let body = &body[..end];

        assert!(!source.contains("fn right_panel_available"));
        assert!(body.contains("render_document_details(cx)"));

        let toggle = source
            .split_once("fn render_right_toggle")
            .expect("render_right_toggle")
            .1;
        let toggle = toggle
            .split("\n    fn render_tabs")
            .next()
            .unwrap_or(toggle);
        assert!(!toggle.contains(".disabled("));
        assert!(toggle.contains(".pressed(self.right_panel_open)"));

        let action = source
            .split_once("fn on_toggle_right_panel")
            .expect("right panel action")
            .1;
        let action = action
            .split("\n    /// Open files dropped")
            .next()
            .unwrap_or(action);
        assert!(!action.contains("right_panel_available"));
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

    /// Nothing in this file may reach the App through the infallible windowed
    /// borrow.
    ///
    /// Source-level because the failure only reproduces when the platform
    /// re-enters the window procedure mid-draw. `update_in` bottoms out in
    /// `AppCell::borrow_mut`, which panics on that re-entrant borrow; every
    /// caller here runs after an `await`, so every caller here can land in one.
    /// `try_update_in` takes the same borrow through `try_borrow_mut` and skips
    /// the frame instead. The literal in the log is `RefCell already borrowed`.
    ///
    /// Checked over the whole file rather than per spawn body: there is no
    /// legitimate `update_in` here, and matching async block extents textually
    /// is how a source check quietly stops asserting anything.
    #[test]
    fn no_async_task_updates_through_the_infallible_borrow() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        // Only the code, not this test's own description of it.
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];

        assert!(
            !code.contains(".update_in("),
            "an update goes through the infallible windowed borrow, which \
             panics when it lands mid-draw. Use `crate::views::try_update_in`."
        );
        assert!(
            code.matches("try_update_in(&this").count() >= 8
                && code.contains("fn prompt_save_as_overwrite"),
            "the folder picker, destructive prompt, Save As picker and Replace \
             confirmation, translation, startup recovery, retirement completion, \
             and cleanup continuation all land after an await and need the \
             fallible path"
        );
        assert!(
            code.contains("try_update(&this, cx, |this, cx| this.drain_watcher(cx))"),
            "the watcher poll lands after an await too; it needs no `Window`, \
             so it takes the windowless fallible path"
        );
    }

    #[test]
    fn save_as_has_an_explicit_workspace_action_and_keybinding() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];
        for action in ["NewDocument", "OpenFile", "OpenFolder"] {
            assert!(
                code.contains(&format!("{action},")),
                "missing {action} action"
            );
        }
        assert!(code.contains("KeyBinding::new(\"ctrl-n\", NewDocument, None)"));
        assert!(code.contains("KeyBinding::new(\"ctrl-o\", OpenFile, None)"));
        assert!(code.contains("KeyBinding::new(\"ctrl-alt-o\", OpenFolder, None)"));
        assert!(
            !code.contains("KeyBinding::new(\"ctrl-o\", OpenFolder, None)"),
            "Ctrl+O must open a file, not only a workspace folder"
        );
        assert!(code.contains("SaveAs,"));
        assert!(code.contains(".on_action(cx.listener(Self::on_save_as))"));
        assert!(code.contains("KeyBinding::new(\"cmd-shift-s\", SaveAs, None)"));
        assert!(code.contains("KeyBinding::new(\"ctrl-shift-s\", SaveAs, None)"));
    }

    /// Auto-refresh must never discard typed text.
    ///
    /// Source-level: reproducing it needs a window, a real editor and a file
    /// changing underneath it. What makes it safe rather than merely working is
    /// that a document refusing to reload — which is what a dirty one does —
    /// still gets flagged, so the conflict banner appears instead. Someone
    /// simplifying the `if !reloaded` away silently loses the banner on exactly
    /// the documents that need it.
    #[test]
    fn auto_refresh_never_reloads_a_document_with_unsaved_edits() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn drain_watcher")
            .expect("drain_watcher must exist");
        let body = &source[start..];
        let end = body.find("\n    // --- Actions").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("watch_auto_reload"),
            "reloading must be opt-in, not the default"
        );
        assert!(
            body.contains("reload_if_clean"),
            "the reload must go through the guard that refuses a dirty document"
        );
        assert!(
            body.contains("if !reloaded") && body.contains("mark_externally_changed"),
            "a document that refused to reload must still be flagged, or the \
             conflict banner never appears and the change goes unnoticed"
        );
        assert!(
            body.contains("let documents = self.document_views()"),
            "one watcher batch must capture the open document set once"
        );
        assert!(
            body.contains(".any(|change| document.watches_path(change.path()))"),
            "each document must inspect the whole batch before one reload or conflict update"
        );
        assert_eq!(
            body.matches("doc.update(cx, |doc, cx| doc.reload_if_clean(cx))")
                .count(),
            1,
            "one watcher batch must start at most one reload per affected document"
        );
        assert_eq!(
            body.matches("doc.update(cx, |doc, cx| doc.mark_externally_changed(cx))")
                .count(),
            1,
            "one watcher batch must mark each affected document at most once"
        );
    }

    /// A translation in flight must show it and refuse a second request.
    ///
    /// Source-level: the failure needs a real network round-trip. Two requests
    /// over the same text race to overwrite the editor with two different
    /// answers, and the flag is only half the guard — the keybindings bypass
    /// the button entirely, and a flag left set by the error path is a
    /// permanently dead button.
    #[test]
    fn a_translation_in_flight_blocks_a_second_one() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        // Only the code: this test names the literals it looks for, and would
        // otherwise count itself.
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];
        let start = code.find("fn translate(").expect("translate must exist");
        let body = &code[start..];
        let end = body.find("\n    // --- Rendering").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("self.translating = true"),
            "the flag must be set before the spawn, or the second click lands \
             before the task starts"
        );
        assert!(
            body.contains("this.translating = false"),
            "and cleared when the result arrives"
        );
        // Cleared once, above the match on the result, rather than per arm —
        // which is what keeps the error path from leaving a dead button.
        let cleared = body.find("this.translating = false").expect("the clear");
        let matched = body.find("match result").expect("the result match");
        assert!(
            cleared < matched,
            "the flag must be cleared for both outcomes; an error path that \
             leaves it set is a permanently dead button"
        );

        // The button goes inert, and so do the keybindings that bypass it.
        assert!(
            code.contains(".loading(self.translating)"),
            "the Translate button must show the request and go inert"
        );
        // Every translate action guards, whatever their number. Pinning the
        // count instead — it was 3 — meant that adding a fourth scope failed a
        // test with nothing to say about the new scope, while a fourth action
        // that forgot its guard could pass as long as some other one had gained
        // a second.
        let mut actions = 0;
        for (at, _) in code.match_indices("fn on_translate_") {
            let body = &code[at..];
            let end = body.find("\n    fn ").unwrap_or(body.len());
            let name: String = body
                .chars()
                .skip("fn ".len())
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert!(
                body[..end].contains("if self.translating {"),
                "{name} must return early while a request is in flight, or a \
                 keybinding starts a second one the inert button cannot"
            );
            actions += 1;
        }
        assert!(actions >= 3, "and the loop above must not be vacuous");
        assert!(
            body.contains("async_snapshot") && body.contains("replace_text_if_current"),
            "a translation result must carry and re-check the exact editor and source identity it read"
        );
        assert!(
            !body.contains("doc.replace_text(translation.text"),
            "the async result must not bypass the source-snapshot gate"
        );
    }

    #[test]
    fn every_close_surface_uses_the_destructive_interlock() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        assert!(source.contains("fn request_destructive("));
        assert!(source.contains("window.on_window_should_close"));

        let action = source
            .split_once("fn on_close_tab")
            .expect("the keyboard action")
            .1
            .split_once("fn on_open_settings")
            .unwrap()
            .0;
        assert!(action.contains("request_close_tab"));

        let tabs = source
            .split_once("fn render_tabs")
            .expect("the tab bar")
            .1
            .split_once("fn render_web_path_controls")
            .unwrap()
            .0;
        assert!(
            tabs.matches("request_close_tab").count() >= 2,
            "both the dirty dot and clean close button must use the same interlock"
        );

        let title_bar = source
            .split_once("fn render_title_bar_backdrop")
            .expect("the Linux title bar")
            .1
            .split_once("fn render_left_title_bar")
            .unwrap()
            .0;
        assert!(title_bar.contains("on_close_window"));
        assert!(title_bar.contains("request_window_close"));
    }

    #[test]
    fn recovery_is_restored_checkpointed_off_thread_and_cleared_intentionally() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        assert!(source.contains("RecoveryStore::open()"));
        assert!(source.contains("store.recover()"));
        assert!(source.contains("restore_startup_recovery("));

        let constructor = source
            .split_once("pub fn new(")
            .expect("the workspace constructor")
            .1
            .split_once("pub fn open_folder")
            .unwrap()
            .0;
        assert!(constructor.contains("start_startup_recovery("));
        assert!(!constructor.contains("let (recovery, recovery_scan"));
        assert!(
            constructor.find("this.open_target(").unwrap()
                < constructor.find("start_startup_recovery(").unwrap(),
            "the requested file must open before startup recovery begins"
        );

        let startup_completion = source
            .split_once("fn restore_startup_recovery")
            .expect("the startup recovery completion path")
            .1
            .split_once("fn flush_pending_recovery_retirements")
            .unwrap()
            .0;
        let pending_cleared = startup_completion
            .find("self.startup_recovery_pending = false")
            .expect("startup pending must clear");
        let applied = startup_completion
            .find("self.restore_prepared_recovery")
            .expect("startup records must be applied");
        let finished = startup_completion
            .find("log::debug!(\"recovery startup finished\")")
            .expect("the content-free startup completion signal");
        let first_return = startup_completion
            .find("return;")
            .expect("the unavailable-store branch");
        assert!(pending_cleared < applied && applied < finished && finished < first_return);
        assert_eq!(
            startup_completion
                .matches("log::debug!(\"recovery startup finished\")")
                .count(),
            1,
            "success, no-record, and error results share one completion signal"
        );

        let checkpoint = source
            .split_once("fn checkpoint_recovery")
            .expect("the recovery scheduler")
            .1
            .split_once("fn finish_recovery_checkpoints")
            .unwrap()
            .0;
        assert!(checkpoint.contains("background_spawn"));
        assert!(checkpoint.contains("recovery_checkpoint(cx)"));
        assert!(checkpoint.contains("checkpoint_batch_if_current"));
        assert!(checkpoint.contains("CancellableRecoveryCheckpointAttempt"));
        assert!(checkpoint.contains("recovery_checkpoint_worker_active"));
        assert!(checkpoint.contains("store_returned_at = background_executor.now()"));
        assert!(
            checkpoint.contains("checkpoint_dispatched(now)"),
            "the durable deadline must be anchored when the editor snapshot is dispatched"
        );
        assert!(
            checkpoint.contains("durable_complete_by"),
            "an in-flight checkpoint must keep its absolute durable deadline observable"
        );

        let completion = source
            .split_once("fn finish_recovery_checkpoints")
            .expect("the recovery completion path")
            .1
            .split_once("fn drain_watcher")
            .unwrap()
            .0;
        assert!(completion.contains("checkpoint_written"));
        assert!(completion.contains("checkpoint_superseded"));
        assert!(completion.contains("checkpoint_failed"));
        assert!(completion.contains("current_checkpoint_write_completed("));
        assert!(completion.contains("state.revision"));
        assert!(completion.contains("attempt.revision"));
        assert!(
            completion.contains("CheckpointBatchOutcome::Written if written_revision_is_current")
        );
        assert!(completion.contains("log::debug!(\"recovery checkpoint written\")"));

        assert!(source.contains("this.retire_document_recovery(id, Some(key), cx);"));
        assert!(
            source.contains("matches!(decision, DirtyDecision::Save | DirtyDecision::Discard)")
        );
        assert!(source.contains("begin_retirement"));
        assert!(source.contains("complete_retirement"));
    }

    #[test]
    fn recovery_activation_registers_created_and_repaired_dirty_schedules() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let arm = source
            .split_once("fn arm_document_recovery_at")
            .expect("the document recovery armer")
            .1
            .split_once("fn schedule_recovery_timer")
            .unwrap()
            .0;
        assert!(arm.contains("Entry::Vacant"));
        assert!(
            arm.matches("activate_and_current_token(&key)").count() >= 2,
            "both an initial schedule and a repaired key must register as active"
        );

        let checkpoint = source
            .split_once("fn checkpoint_recovery_at")
            .expect("the recovery scheduler")
            .1
            .split_once("fn finish_recovery_checkpoints")
            .unwrap()
            .0;
        assert!(checkpoint.contains(".or_insert_with(||"));
        assert!(
            checkpoint
                .matches("activate_and_current_token(&key)")
                .count()
                >= 2,
            "a missing schedule and a repaired key in the fallback must register as active"
        );
    }

    /// Search must never search a stale copy of an open document, and must
    /// never walk the filesystem on the UI thread.
    ///
    /// Source-level: reproducing either needs a window, an editor and a real
    /// vault. Both failures are real and were measured — walking the 6,642-file
    /// vault takes 2.4s, which as a UI-thread cost is a frozen window on every
    /// settled keystroke; and reading an open tab from disk misses everything
    /// typed since the last save.
    #[test]
    fn the_search_corpus_is_cheap_to_build_and_prefers_editor_text() {
        let source = crate::views::production_source(include_str!("workspace.rs"));
        let start = source
            .find("fn search_corpus")
            .expect("search_corpus must exist");
        let body = &source[start..];
        let end = body
            .find("\n    /// Paths of every open tab")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("corpus.open.push"),
            "open tabs must contribute their editor text"
        );
        assert!(
            !body.contains("document_paths("),
            "walking a directory here runs on the UI thread — measured at 2.4s \
             on a 6,642-document vault. Hand the directory over in \
             `Corpus::roots` and let the background task walk it."
        );
        assert!(
            body.contains("corpus.roots"),
            "directories must be passed unwalked"
        );
        assert!(
            body.contains("!open.contains(p)"),
            "files already contributed as editor text must be filtered out, or \
             every match in an open document is reported twice"
        );
        assert!(
            body.contains("skills().iter().map(|s| s.dir.clone())"),
            "the Harness scope must cover a skill's whole directory, not only \
             its SKILL.md — references and scripts are part of the skill"
        );
    }
}
