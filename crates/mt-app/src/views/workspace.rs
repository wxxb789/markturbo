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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::{Button as BaseButton, GlobalState, Toggle as BaseToggle};
use gpui_component::{
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
use mt_doc::translate::Scope;

use crate::fs;
use crate::i18n;
use crate::metrics;
use crate::renderer::RendererRegistry;
use crate::translate::Provider;
use crate::views::document::{DocumentEvent, DocumentView};
use crate::views::explorer::{Explorer, ExplorerEvent};
use crate::views::harness::{HarnessEvent, HarnessView};
use crate::views::search::{Corpus, SearchEvent, SearchView};
use crate::views::settings_page::{SettingsEvent, SettingsView};
use crate::views::tabs::Tabs;
use crate::watcher::Watcher;

mod history;
pub(crate) mod web_surface;

use self::history::History;
use self::web_surface::WebSurface;

actions!(
    markturbo,
    [
        OpenFolder,
        Save,
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

pub struct Workspace {
    focus_handle: FocusHandle,
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
    _tasks: Vec<Task<()>>,
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
            web: WebSurface::default(),
            _tasks: vec![poll],
            _subscriptions: Vec::new(),
            _panel_subscriptions: Vec::new(),
        };

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

        // A path argument may name a file: open its parent as the workspace and
        // the file as the first tab, which is what "open this file with
        // markturbo" should do.
        if let Some(path) = initial {
            let (root, file) = if path.is_dir() {
                (Some(path), None)
            } else {
                (path.parent().map(Path::to_path_buf), Some(path))
            };
            if let Some(root) = root {
                this.open_folder(root, window, cx);
            }
            if let Some(file) = file.filter(|f| f.is_file()) {
                this.open_file(file, window, cx);
            }
        }
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
        // Any results on screen came from the folder that was open a moment
        // ago. Leaving them would present another project's matches as this
        // one's, which is worse than an empty list.
        let search = self.search.clone();
        search.update(cx, |search, cx| search.rerun(cx));
        cx.notify();
    }

    /// Open a file in a tab, focusing an existing tab if it is already open.
    ///
    /// A pinned open: the tab stays until the user closes it.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_as(path, false, window, cx);
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
    ) {
        // Opening the file that is already the preview, by double click, is how
        // it gets promoted — the tab is already right, only its status changes.
        if preview {
            if self.tabs.is_preview(&path) {
                self.focus_path(&path, cx);
                return;
            }
        } else if self.tabs.is_preview(&path) {
            self.tabs.set_preview(None);
            self.focus_path(&path, cx);
            return;
        }

        // Replace the outgoing preview rather than accumulating tabs. Its edits
        // are the one thing that must not be discarded silently, so a dirty
        // preview is kept and simply stops being one.
        if preview
            && let Some(current) = self.tabs.take_preview()
            && let Some(ix) = self.tabs.index_of(&current)
        {
            if self
                .tabs
                .get(ix)
                .is_some_and(|t| t.payload.view.read(cx).is_dirty())
            {
                // Keep it: promoting beats losing unsaved work.
            } else {
                self.close_tab(ix, cx);
            }
        }

        self.open_file_inner(path.clone(), window, cx);
        self.tabs.set_preview(preview.then_some(path));
        cx.notify();
    }

    /// Focus the tab showing `path`, if it is open.
    fn focus_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.tabs.focus_path(path) {
            self.record_visit(path.to_path_buf(), 0);
            self.web_dirty(cx);
            cx.notify();
        }
    }

    fn open_file_inner(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Opening a document while settings are showing has to show the
        // document, or the click in the explorer looks like it did nothing.
        self.settings_open = false;

        if self.tabs.focus_path(&path) {
            self.record_visit(path, 0);
            self.web_dirty(cx);
            cx.notify();
            return;
        }

        let file = match fs::load(&path) {
            Ok(file) => file,
            Err(err) => {
                self.set_status(format!("Cannot open {}: {err}", path.display()), cx);
                return;
            }
        };

        let registry = self.registry.clone();
        let view = cx.new(|cx| DocumentView::new(file, registry, window, cx));

        // Both subscriptions ride with the tab, so closing it drops them.
        let subscriptions = [
            cx.subscribe_in(
                &view,
                window,
                |this: &mut Self, _, event: &DocumentEvent, _, cx| match event {
                    DocumentEvent::Status(message) => this.set_status(message.clone(), cx),
                    DocumentEvent::Conflict => this.set_status(
                        "This file changed on disk. Reload or overwrite from the banner.".into(),
                        cx,
                    ),
                    DocumentEvent::DirtyChanged => cx.notify(),
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

        self.tabs.push(
            path.clone(),
            DocumentTab {
                view,
                _subscriptions: subscriptions,
            },
        );
        self.record_visit(path, 0);
        self.web_dirty(cx);
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        // ponytail: closing a dirty tab drops its edits. A confirm dialog is
        // the obvious next step; the file on disk is never touched either way.
        //
        // `Tabs::close` also shifts the active index, empties the preview slot
        // if it named this tab, and drops the tab's two subscriptions with it.
        let Some((closed, _dropped)) = self.tabs.close(ix) else {
            return;
        };
        // Otherwise Back reopens the tab that was just closed, which reads as
        // the close button not working.
        self.history.forget(&closed);
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
        let changes = watcher.poll();
        if changes.is_empty() {
            return;
        }

        let tree_changed = changes.iter().any(|c| c.affects_tree());
        let removed_artifact = self.harness.as_ref().is_some_and(|harness| {
            let harness = harness.read(cx);
            changes
                .iter()
                .any(|change| change.affects_tree() && harness.has_artifact_under(change.path()))
        });
        let harness_changed = removed_artifact
            || changes
                .iter()
                .any(|change| path_affects_harness(watcher.root(), change.path()));

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
        let auto_reload = crate::settings::AppSettings::global(cx).watch_auto_reload;
        for change in &changes {
            let path = change.path();
            // Cloned: `self.documents` cannot stay borrowed across the `&mut cx`
            // that leasing each entity takes.
            for doc in self.document_views() {
                if doc.read(cx).path() != path {
                    continue;
                }
                let reloaded = auto_reload && doc.update(cx, |doc, cx| doc.reload_if_clean(cx));
                if !reloaded {
                    doc.update(cx, |doc, cx| doc.mark_externally_changed(cx));
                }
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

    fn on_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open workspace folder".into()),
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
                this.open_folder(path, window, cx);
            });
        })
        .detach();
    }

    fn on_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_document().cloned() {
            doc.update(cx, |doc, cx| doc.save(false, cx));
        }
    }

    fn on_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.tabs.active_index(), cx);
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
                self.open_folder(path.clone(), window, cx);
                opened += 1;
            } else if crate::workspace::is_openable(path) {
                // With no folder open, adopt the file's parent as the
                // workspace — the same thing a path argument does, and it is
                // what makes the tree useful after a bare drop.
                if self.root.is_none()
                    && let Some(parent) = path.parent()
                {
                    self.open_folder(parent.to_path_buf(), window, cx);
                }
                self.open_file(path.clone(), window, cx);
                opened += 1;
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
        self.tabs.menu_target().map(|t| t.path.clone())
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
        let text = doc.read(cx).text(cx);
        let doc_type = doc.read(cx).document().doc_type();

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

            // `try_update_in` rather than `update_in`: this lands after an
            // await and can arrive mid-draw, where the infallible borrow
            // panics. Skipping costs one frame; panicking costs the session.
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                // Cleared in both arms — a flag left set by the error path is a
                // permanently dead button.
                this.translating = false;
                match result {
                    Ok(translation) => {
                        doc.update(cx, |doc, cx| {
                            doc.replace_text(translation.text, window, cx);
                        });
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
                    }
                    Err(err) => this.set_status(format!("Translation failed: {err}"), cx),
                }
            });
        })
        .detach();
    }

    // --- Rendering --------------------------------------------------------

    fn render_document_details(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (title, kind, location, status) = {
            let document = self.active_document()?.read(cx);
            let location = self
                .root
                .as_deref()
                .filter(|root| document.path().starts_with(root))
                .map(|root| crate::workspace::display_relative(root, document.path()))
                .unwrap_or_else(|| document.path().to_string_lossy().replace('\\', "/"));
            let status = i18n::t(
                if document.is_dirty() {
                    i18n::Key::UnsavedChanges
                } else {
                    i18n::Key::Saved
                },
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
        self.record_visit(doc.read(cx).path().to_path_buf(), offset);
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
        if doc.read(cx).path() != path {
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
                corpus.open.push((tab.path.clone(), doc.text(cx)));
            }
        };

        match self.search.read(cx).scope() {
            Scope::Document => {
                if let Some(doc) = self.active_document() {
                    let doc = doc.read(cx);
                    corpus.open.push((doc.path().to_path_buf(), doc.text(cx)));
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
                let path = tab.path.clone();
                let full = path.to_string_lossy().replace('\\', "/");
                // Relative only makes sense with a folder open, and only for a
                // file actually under it — a globally-discovered skill is not.
                let relative = root
                    .as_ref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .map(|rest| rest.to_string_lossy().replace('\\', "/"));
                let is_preview = self.tabs.is_preview(&path);
                let dirty = doc.is_dirty();
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
                    .when(!web_active, |tab| {
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
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| this.close_tab(ix, cx)))
                                .into_any_element()
                        },
                    )
            }))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if this.tabs.focus(*ix)
                    && let Some(tab) = this.tabs.get(*ix)
                {
                    let path = tab.path.clone();
                    this.record_visit(path, 0);
                }
                this.web_dirty(cx);
                cx.notify();
            }))
    }

    fn render_web_path_controls(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.web_active(cx) {
            return None;
        }
        let path = self.active_document()?.read(cx).path().to_path_buf();
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
            .child(
                ChromeIconButton::new(
                    "open-folder",
                    IconName::FolderOpen,
                    i18n::t(i18n::Key::OpenFolder, cx),
                )
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::OpenFolder, cx))
                })
                .on_click(
                    cx.listener(|this, _, window, cx| this.on_open_folder(&OpenFolder, window, cx)),
                ),
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
            .child(self.render_left_toggle(tooltips, cx))
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
        TitleBar::new()
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
            .children(self.status.clone().map(|s| div().flex_1().child(s)))
            .when(self.status.is_none(), |this| {
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
                        cx.notify();
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
        let right_panel = self.render_right_panel(cx);
        let right_panel_visible = right_panel.is_some();
        let side_panel = self
            .left_panel_open
            .then(|| self.render_side_panel(cx).into_any_element());
        let panel_widths = self.workspace_panel_widths(viewport, right_panel_visible);
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
                !self.left_panel_open,
                !right_panel_visible,
                cx,
            )
            .into_any_element();
        let left_title_region = self.left_panel_open.then(|| {
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
                    self.left_panel_open,
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
            .key_context("Workspace")
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_save))
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
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("ctrl-o", OpenFolder, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
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
    use std::{cell::RefCell, path::Path, rc::Rc};

    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{
        DetailsContent, SidePanel, TAB_LABEL_MAX, Workspace, WorkspaceResizeEdge,
        clamped_dragged_panel_width, details_content, elide_tab_label, path_affects_harness,
        resolved_workspace_panel_widths,
    };
    use gpui::{AppContext as _, Modifiers, MouseButton, TestAppContext, point, px};

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
            .find(".when(!web_active, |tab|")
            .expect("the Web-active overlay gate");
        let end = body[gate..]
            .find("// A preview tab")
            .map(|end| gate + end)
            .unwrap_or(body.len());
        let affordance = &body[gate..end];

        assert!(affordance.contains(".tooltip(") && affordance.contains(".context_menu("));
        assert!(affordance.contains("tab.child("));
        assert!(affordance.contains(".absolute()") && affordance.contains(".inset_0()"));
        assert!(!body.contains(".prefix("));
        assert!(body.contains(".aria_label(aria_label)"));
        assert!(
            body.contains(".when(!web_active, |this|") && body.contains("UnsavedChanges"),
            "the dirty marker tooltip needs its own Web-active gate"
        );
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
        for key in ["OpenFolder", "Translate", "Settings"] {
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
        assert!(render.contains("let right_panel = self.render_right_panel(cx)"));
        let side_panel = &render[render.find("let side_panel").expect("side panel")..];
        let side_panel = &side_panel[..side_panel
            .find("let panel_widths")
            .unwrap_or(side_panel.len())];
        assert!(side_panel.contains(".left_panel_open") && side_panel.contains(".then("));
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
        let end = body.find("\n    /// Apply pending").unwrap_or(body.len());
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
            body.contains("self.root.is_none()"),
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
        let end = tabs
            .find("\n    fn render_web_path_controls")
            .unwrap_or(tabs.len());
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
            body.contains("self.left_panel_open.then(||") && body.contains("left-title-region"),
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
        assert_eq!(
            code.matches("try_update_in(&this").count(),
            2,
            "the folder prompt and the translation both land after an await and \
             both need the fallible path"
        );
        assert!(
            code.contains("try_update(&this, cx, |this, cx| this.drain_watcher(cx))"),
            "the watcher poll lands after an await too; it needs no `Window`, \
             so it takes the windowless fallible path"
        );
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
