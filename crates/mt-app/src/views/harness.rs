//! The Harness panel: skills and instruction files.
//!
//! Both are agent artifacts and both are discovered from the same harness
//! conventions, so they belong in one panel rather than two. A skill is a
//! directory with a `SKILL.md`; an instruction file is what a harness reads
//! unprompted — `CLAUDE.md`, `AGENTS.md`, a Cursor rule. Selecting a skill
//! exposes its metadata, validation state, and files; selecting either opens the
//! underlying document.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mt_core::agent_artifacts::{instruction, skill};
use mt_core::{Instruction, Origin, Severity, Skill};

use crate::i18n;
use crate::metrics;
use crate::settings::AppSettings;
use mt_core::agent_artifacts::context::{
    CODEX_PROFILE_ID, ContextInput, InclusionRule, ProjectTrust, ResolutionStatus, ResolvedContext,
    SourceOccurrence,
};
use mt_core::settings::GroupBy;

/// Emitted when the user wants to open an artifact's document.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    /// Open `path`. `preview` means a single click: the tab is transient and
    /// the next single click replaces it, which is what keeps browsing a list
    /// from leaving a bar full of tabs.
    OpenFile { path: PathBuf, preview: bool },
    /// The scenario inputs changed. Workspace clears per-request source choices
    /// and resolves this exact snapshot off the UI thread; `None` means a field
    /// is not currently parseable and must not reuse an older result.
    ContextChanged { input: Option<ContextInput> },
}

/// Which kind of artifact the panel is listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Skills,
    Instructions,
    Context,
}

impl Section {
    const ALL: [Section; 3] = [Section::Skills, Section::Instructions, Section::Context];

    fn label(self) -> Option<crate::i18n::Key> {
        match self {
            Section::Skills => Some(crate::i18n::Key::SectionSkills),
            Section::Instructions => Some(crate::i18n::Key::SectionInstructions),
            Section::Context => None,
        }
    }
}

/// Stable identity for an instruction occurrence, independent of chain order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextSourceIdentity {
    path: PathBuf,
    scope: PathBuf,
    origin: Origin,
    rule: InclusionRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexHomeSource {
    EnvironmentOverride,
    UserHomeDefault,
    HomeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextProfileChoice {
    profile_id: String,
    supported: bool,
}

impl SearchableListItem for ContextProfileChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        format!(
            "{} — {}",
            self.profile_id,
            if self.supported {
                "verified AGENTS instructions"
            } else {
                "inventory only"
            }
        )
        .into()
    }

    fn value(&self) -> &Self::Value {
        &self.profile_id
    }
}

impl From<&SourceOccurrence> for ContextSourceIdentity {
    fn from(source: &SourceOccurrence) -> Self {
        Self {
            path: source.path.clone(),
            scope: source.scope.clone(),
            origin: source.origin,
            rule: source.rule,
        }
    }
}

/// Keep the current section when it still has rows, otherwise expose the other
/// artifact kind instead of leaving the details switch permanently disabled.
fn populated_section(preferred: Section, skills: usize, instructions: usize) -> Option<Section> {
    match preferred {
        Section::Skills if skills > 0 => Some(Section::Skills),
        Section::Instructions if instructions > 0 => Some(Section::Instructions),
        _ if skills > 0 => Some(Section::Skills),
        _ if instructions > 0 => Some(Section::Instructions),
        _ => None,
    }
}

pub struct HarnessView {
    focus_handle: FocusHandle,
    root: PathBuf,
    skill_cache: Arc<Mutex<skill::DiscoveryCache>>,
    section: Section,
    skills: Vec<Skill>,
    instructions: Vec<Instruction>,
    selected: Option<usize>,
    target_input: Entity<InputState>,
    cwd_input: Entity<InputState>,
    profile_select: Entity<SelectState<SearchableVec<ContextProfileChoice>>>,
    codex_home_input: Entity<InputState>,
    fallback_input: Entity<InputState>,
    byte_budget_input: Entity<InputState>,
    root_markers_input: Entity<InputState>,
    project_trust: ProjectTrust,
    codex_home_default: PathBuf,
    codex_home_source: CodexHomeSource,
    context_input: Option<ContextInput>,
    context_error: Option<String>,
    resolved_context: Option<ResolvedContext>,
    selected_context_source: Option<ContextSourceIdentity>,
    context_pending: bool,
    _context_subscriptions: Vec<Subscription>,
    /// True while a scan is in flight — which is both what keeps an empty list
    /// from reading as "nothing installed" before the first scan lands, and
    /// what spins the rescan button.
    scanning: bool,
    _scan: Option<Task<()>>,
}

/// The shortest time the rescan button stays in its loading state.
///
/// Discovery over a small workspace returns in a few milliseconds, so the
/// spinner would appear and vanish inside a frame or two — which reads as a
/// glitch rather than as feedback, and leaves the "did my click register?"
/// question the button exists to answer still unanswered. Same 250ms as the
/// search debounce: a shorter state change is not perceived as one.
const SPINNER_FLOOR: Duration = Duration::from_millis(250);

impl HarnessView {
    pub fn new(
        root: PathBuf,
        skill_cache: Arc<Mutex<skill::DiscoveryCache>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (codex_home, codex_home_source) = default_codex_home();
        let codex_home_default = codex_home.clone();
        let target_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(root.to_string_lossy().to_string())
        });
        let cwd_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(root.to_string_lossy().to_string())
        });
        let profile_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(context_profile_choices()),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
            .searchable(true)
        });
        let codex_home_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(codex_home.to_string_lossy().to_string())
        });
        let fallback_input = cx.new(|cx| InputState::new(window, cx));
        let byte_budget_input = cx.new(|cx| InputState::new(window, cx).default_value("32768"));
        let root_markers_input = cx.new(|cx| InputState::new(window, cx).default_value(".git"));
        let mut context_subscriptions: Vec<Subscription> = [
            target_input.clone(),
            cwd_input.clone(),
            codex_home_input.clone(),
            fallback_input.clone(),
            byte_budget_input.clone(),
            root_markers_input.clone(),
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.context_input_changed(cx);
                }
            })
        })
        .collect();
        context_subscriptions.push(cx.subscribe(
            &profile_select,
            |this, _, event: &SelectEvent<SearchableVec<ContextProfileChoice>>, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    this.context_input_changed(cx);
                }
            },
        ));

        let context_input = ContextInput {
            workspace: root.clone(),
            target: root.clone(),
            cwd: root.clone(),
            profile_id: CODEX_PROFILE_ID.to_string(),
            codex_home,
            codex_home_override: codex_home_source == CodexHomeSource::EnvironmentOverride,
            fallback_filenames: Vec::new(),
            project_doc_max_bytes: 32_768,
            project_root_markers: vec![".git".to_string()],
            project_trust: ProjectTrust::Unspecified,
        };
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root,
            skill_cache,
            section: Section::Skills,
            skills: Vec::new(),
            instructions: Vec::new(),
            selected: None,
            target_input,
            cwd_input,
            profile_select,
            codex_home_input,
            fallback_input,
            byte_budget_input,
            root_markers_input,
            project_trust: ProjectTrust::Unspecified,
            codex_home_default,
            codex_home_source,
            context_input: Some(context_input),
            context_error: None,
            resolved_context: None,
            selected_context_source: None,
            context_pending: false,
            _context_subscriptions: context_subscriptions,
            scanning: true,
            _scan: None,
        };
        this.refresh(cx);
        this
    }

    /// Rediscover skills and instruction files from disk, off the UI thread.
    ///
    /// Discovery covers every harness's workspace directory plus the global
    /// ones, and it runs on a filesystem-watcher tick — doing that synchronously
    /// would stutter the window on every save. Both scans share one task: they
    /// walk overlapping directories, so running them together keeps the
    /// filesystem cache warm and halves the notify traffic.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let settings = AppSettings::global(cx);
        let global = settings.skills_include_global;
        let options = mt_core::Discovery {
            global,
            include_internal: settings.skills_include_internal,
        };
        let skill_cache = self.skill_cache.clone();
        self.scanning = true;
        // Replacing the task cancels any scan still in flight, so a burst of
        // filesystem events costs one scan rather than one per event.
        self._scan = Some(cx.spawn(async move |this, cx| {
            // Started before the scan, so the floor is measured from the click
            // rather than from the moment the results happen to land.
            let floor = cx.background_executor().timer(SPINNER_FLOOR);
            let found = cx
                .background_spawn(async move {
                    let skills = {
                        let mut cache = skill_cache
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        skill::discover_with_cache(&root, options, &mut cache)
                    };
                    (skills, instruction::discover_with(&root, global))
                })
                .await;
            crate::views::try_update(&this, cx, |this, cx| this.apply(found.0, found.1, cx));
            // Only the spinner waits out the floor; the lists above are already
            // on screen.
            floor.await;
            crate::views::try_update(&this, cx, |this, cx| {
                this.scanning = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Force every global root to be read again.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.skill_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.refresh(cx);
    }

    fn apply(
        &mut self,
        skills: Vec<Skill>,
        instructions: Vec<Instruction>,
        cx: &mut Context<Self>,
    ) {
        // Remember the selection by identity, not index: a rediscovery can
        // reorder either list.
        let previous = self.selected_path();
        self.skills = skills;
        self.instructions = instructions;
        self.selected = previous.and_then(|path| self.position_of(&path));
        if self.selected.is_none() && self.section != Section::Context {
            self.section =
                populated_section(self.section, self.skills.len(), self.instructions.len())
                    .unwrap_or(self.section);
            self.selected = (!self.is_empty()).then_some(0);
        }
        cx.notify();
    }

    /// The path identifying the current selection, whichever section is showing.
    fn selected_path(&self) -> Option<PathBuf> {
        let ix = self.selected?;
        match self.section {
            Section::Skills => self.skills.get(ix).map(|s| s.dir.clone()),
            Section::Instructions => self.instructions.get(ix).map(|i| i.path.clone()),
            Section::Context => self
                .selected_context_source
                .as_ref()
                .map(|source| source.path.clone()),
        }
    }

    fn position_of(&self, path: &Path) -> Option<usize> {
        match self.section {
            Section::Skills => self.skills.iter().position(|s| s.dir == path),
            Section::Instructions => self.instructions.iter().position(|i| i.path == path),
            Section::Context => None,
        }
    }

    fn is_empty(&self) -> bool {
        match self.section {
            Section::Skills => self.skills.is_empty(),
            Section::Instructions => self.instructions.is_empty(),
            Section::Context => self
                .resolved_context
                .as_ref()
                .is_none_or(|context| context.sources.is_empty()),
        }
    }

    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
        if section == Section::Context {
            cx.notify();
            return;
        }
        // The selection indexes into whichever list was showing, so it cannot
        // carry across. Selecting the first row beats leaving the inspector on
        // an artifact the list no longer contains.
        self.selected = (!self.is_empty()).then_some(0);
        cx.notify();
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }

    /// The current validated input snapshot consumed by Workspace's resolver.
    pub fn context_input(&self) -> Option<&ContextInput> {
        self.context_input.as_ref()
    }

    /// The latest result, only while it still matches the current input identity.
    pub fn resolved_context(&self) -> Option<&ResolvedContext> {
        self.resolved_context.as_ref()
    }

    /// Whether Workspace is already resolving the current scenario.
    pub fn context_is_pending(&self) -> bool {
        self.context_pending
    }

    /// The visible scenario target. Opening a context source does not change it.
    pub fn selected_target(&self, cx: &App) -> PathBuf {
        PathBuf::from(self.target_input.read(cx).value().to_string())
    }

    /// Mark the exact input snapshot as pending in Workspace's background resolver.
    pub fn set_context_pending(&mut self, pending: bool, cx: &mut Context<Self>) {
        self.context_pending = pending;
        cx.notify();
    }

    /// Accept only results for the exact current input snapshot.
    pub fn apply_resolved_context(
        &mut self,
        context: ResolvedContext,
        cx: &mut Context<Self>,
    ) -> bool {
        if !context_matches_input(self.context_input.as_ref(), &context) {
            return false;
        }
        let previous = self.selected_context_source.clone();
        self.selected_context_source = previous
            .filter(|key| {
                context
                    .sources
                    .iter()
                    .any(|source| ContextSourceIdentity::from(source) == *key)
            })
            .or_else(|| context.sources.first().map(ContextSourceIdentity::from));
        self.resolved_context = Some(context);
        self.context_pending = false;
        cx.notify();
        true
    }

    fn build_context_input(&self, cx: &App) -> Result<ContextInput, String> {
        let byte_budget = self
            .byte_budget_input
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
            .map_err(|_| "Project byte budget must be a non-negative integer".to_string())?;
        Ok(ContextInput {
            workspace: self.root.clone(),
            target: PathBuf::from(self.target_input.read(cx).value().to_string()),
            cwd: PathBuf::from(self.cwd_input.read(cx).value().to_string()),
            profile_id: self
                .profile_select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default(),
            codex_home: PathBuf::from(self.codex_home_input.read(cx).value().to_string()),
            codex_home_override: self.codex_home_is_override(cx),
            fallback_filenames: split_config_names(self.fallback_input.read(cx).value().as_ref()),
            project_doc_max_bytes: byte_budget,
            project_root_markers: split_config_names(
                self.root_markers_input.read(cx).value().as_ref(),
            ),
            project_trust: self.project_trust,
        })
    }

    fn codex_home_is_override(&self, cx: &App) -> bool {
        codex_home_is_override(
            self.codex_home_source,
            &self.codex_home_default,
            &PathBuf::from(self.codex_home_input.read(cx).value().to_string()),
        )
    }

    fn context_input_changed(&mut self, cx: &mut Context<Self>) {
        let (input, error) = match self.build_context_input(cx) {
            Ok(input) => (Some(input), None),
            Err(error) => (None, Some(error)),
        };
        self.context_input = input.clone();
        self.context_error = error;
        self.resolved_context = None;
        self.selected_context_source = None;
        self.context_pending = input.is_some();
        cx.emit(HarnessEvent::ContextChanged { input });
        cx.notify();
    }

    /// Whether the current result set contains an artifact below `path`.
    ///
    /// A removed or renamed-out directory no longer answers `is_dir()`. The
    /// watcher asks the last successful scan instead, so dotted directory names
    /// remain distinguishable from ordinary removed files without restoring the
    /// broad refresh-on-every-tree-change behavior.
    pub fn has_artifact_under(&self, path: &Path) -> bool {
        artifacts_under(&self.skills, &self.instructions, path)
    }

    /// Redraw without rescanning.
    ///
    /// `selected` is an index into `self.skills`, which regrouping does not
    /// reorder — only the rendered order changes — so the selection survives on
    /// its own and this is just a notify with a name that says why.
    fn keep_selection_stable(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// The document behind row `ix` of the current section.
    fn entry_path(&self, ix: usize) -> Option<PathBuf> {
        match self.section {
            Section::Skills => self.skills.get(ix).map(|s| s.entry.clone()),
            Section::Instructions => self.instructions.get(ix).map(|i| i.path.clone()),
            Section::Context => None,
        }
    }

    fn selected_skill(&self) -> Option<&Skill> {
        (self.section == Section::Skills)
            .then(|| self.selected.and_then(|ix| self.skills.get(ix)))
            .flatten()
    }

    fn selected_instruction(&self) -> Option<&Instruction> {
        (self.section == Section::Instructions)
            .then(|| self.selected.and_then(|ix| self.instructions.get(ix)))
            .flatten()
    }

    /// The list of instruction files.
    ///
    /// Flat rather than grouped: there are a handful of these, not a hundred,
    /// and the origin heading is the only grouping that would apply — which the
    /// per-row origin badge already carries.
    fn render_instructions(&self, cx: &Context<Self>) -> AnyElement {
        if self.instructions.is_empty() {
            let hint = if self.scanning {
                i18n::t(i18n::Key::Scanning, cx).to_string()
            } else {
                format!(
                    "No instruction files found.\n\nSearched {} workspace \
                     directories (the root, .claude, .cursor, .github, …) for \
                     AGENTS.md, CLAUDE.md, rules and scoped instructions.",
                    instruction::project_roots().len(),
                )
            };
            return v_flex()
                .p(metrics::inset())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
                .into_any_element();
        }

        v_flex()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .children(self.instructions.iter().enumerate().map(|(ix, entry)| {
                let selected = self.selected == Some(ix);
                ListItem::new(("instruction", ix))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_1()
                    .rounded(cx.theme().radius)
                    .selected(selected)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Icon::new(IconName::BookOpen).small())
                                    .child(div().flex_1().text_sm().truncate().child(entry.label()))
                                    .when(!entry.aliases.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("+{}", entry.aliases.len())),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(entry.doc_type.label())
                                    .child(entry.origin.label()),
                            ),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.selected = Some(ix);
                        if let Some(path) = this.entry_path(ix) {
                            cx.emit(HarnessEvent::OpenFile {
                                path,
                                preview: event.click_count() < 2,
                            });
                        }
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    /// The inspector for the selected instruction file.
    ///
    /// Thinner than the skill inspector on purpose: an instruction file has no
    /// schema to validate against, so what is worth showing is where it came
    /// from and a way to open it.
    fn render_instruction_inspector(&self, cx: &Context<Self>) -> AnyElement {
        let Some(entry) = self.selected_instruction() else {
            return div().into_any_element();
        };
        let path = entry.path.clone();

        v_flex()
            .p(metrics::inset())
            .gap(metrics::gap())
            .child(
                h_flex()
                    .gap(metrics::gap())
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .child(entry.label()),
                    )
                    .child(
                        Button::new("open-instruction")
                            .label(i18n::t(i18n::Key::Open, cx))
                            .xsmall()
                            .primary()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HarnessEvent::OpenFile {
                                    path: path.clone(),
                                    preview: false,
                                });
                            })),
                    ),
            )
            .children(field(
                cx,
                i18n::t(i18n::Key::Kind, cx),
                entry.doc_type.label(),
            ))
            .children(field(
                cx,
                i18n::t(i18n::Key::Origin, cx),
                entry.origin.label(),
            ))
            .children(field(
                cx,
                i18n::t(i18n::Key::Location, cx),
                &located(&self.root, entry.origin, &entry.path),
            ))
            .children((!entry.aliases.is_empty()).then(|| {
                v_flex()
                    .gap_0p5()
                    .child(label(cx, i18n::t(i18n::Key::AlsoLinkedFrom, cx)))
                    .children(entry.aliases.iter().map(|alias| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(located(&self.root, entry.origin, alias))
                    }))
            }))
            .into_any_element()
    }

    fn render_context_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = v_flex()
            .id("harness-context")
            .p(metrics::inset())
            .gap(metrics::gap())
            .child(label(cx, "Effective context scenario"))
            .child(context_input_field(
                &self.target_input,
                "Target path",
                "harness-context-target",
                "MarkTurbo scenario target; editing this does not change Codex discovery rules.",
                cx,
            ))
            .child(
                h_flex()
                    .justify_end()
                    .child(
                        Button::new("harness-context-use-target-parent-as-cwd")
                            .label("Use target parent as cwd")
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let target = PathBuf::from(
                                    this.target_input.read(cx).value().to_string(),
                                );
                                let Some(cwd) = target_parent_as_cwd(&target) else {
                                    return;
                                };
                                let cwd = cwd.to_string_lossy().to_string();
                                if this.cwd_input.read(cx).value().as_ref() == cwd {
                                    return;
                                }
                                this.cwd_input.update(cx, |input, cx| {
                                    input.set_value(cwd, window, cx);
                                });
                                // InputState::set_value intentionally emits no Change event.
                                this.context_input_changed(cx);
                            })),
                    ),
            )
            .child(context_input_field(
                &self.cwd_input,
                "Execution cwd",
                "harness-context-cwd",
                "Codex project instruction discovery follows this directory, not target ancestors.",
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("For a file target this sets its parent as MarkTurbo scenario cwd; for a directory target, set cwd to that directory."),
            )
            .child(context_profile_selector(&self.profile_select, cx))
            .child(context_input_field(
                &self.codex_home_input,
                "CODEX_HOME",
                "harness-context-codex-home",
                "Editable local scenario input; the resolver checks membership and reports invalid overrides.",
                cx,
            ))
            .child(context_input_field(
                &self.fallback_input,
                "Project fallback filenames",
                "harness-context-fallback-filenames",
                "Separate names with commas; empty names are ignored by the profile.",
                cx,
            ))
            .child(context_input_field(
                &self.byte_budget_input,
                "Project document byte budget",
                "harness-context-byte-budget",
                "Raw project bytes only; global instructions and separators are excluded.",
                cx,
            ))
            .child(context_input_field(
                &self.root_markers_input,
                "Project root markers",
                "harness-context-root-markers",
                "Separate marker names with commas; an empty list uses the cwd only.",
                cx,
            ))
            .child(
                v_flex()
                    .gap(metrics::row_gap())
                    .child(label(cx, "Project trust"))
                    .child(
                        h_flex()
                            .gap(metrics::gap())
                            .children(
                                [
                                    ProjectTrust::Unspecified,
                                    ProjectTrust::Trusted,
                                    ProjectTrust::Untrusted,
                                ]
                                .into_iter()
                                .map(|trust| {
                                    Button::new(format!("harness-context-trust-{trust:?}"))
                                        .label(project_trust_label(trust))
                                        .xsmall()
                                        .when(self.project_trust == trust, |button| {
                                            button.primary()
                                        })
                                        .when(self.project_trust != trust, |button| {
                                            button.ghost()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.project_trust != trust {
                                                this.project_trust = trust;
                                                this.context_input_changed(cx);
                                            }
                                        }))
                                }),
                            ),
                    ),
            )
            .child(self.render_context_summary(cx));

        if let Some(error) = &self.context_error {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }

        let Some(context) = &self.resolved_context else {
            let message = if self.context_pending {
                "Waiting for Workspace to resolve these inputs."
            } else {
                "No result is available for these inputs."
            };
            return content
                .child(context_section_heading(cx, "Effective instruction chain"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(message),
                )
                .into_any_element();
        };

        content = content
            .child(context_section_heading(cx, "Effective instruction chain"))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} · {} occurrence(s)",
                        resolution_status_label(context.status),
                        context.sources.len()
                    )),
            );

        content = content.children(context.sources.iter().enumerate().map(|(index, source)| {
            let identity = ContextSourceIdentity::from(source);
            let selected = self.selected_context_source.as_ref() == Some(&identity);
            let duplicate_count = source.content_index.map_or(0, |content_index| {
                context
                    .sources
                    .iter()
                    .filter(|other| other.content_index == Some(content_index))
                    .count()
                    .saturating_sub(1)
            });
            ListItem::new(context_source_element_id(source))
                .w_full()
                .px(metrics::row_pad())
                .py_1()
                .rounded(cx.theme().radius)
                .selected(selected)
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap(metrics::gap())
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("{}.", index + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .truncate()
                                        .child(source.path.display().to_string()),
                                )
                                .when(source.is_truncated(), |this| {
                                    this.child(
                                        Icon::new(IconName::TriangleAlert)
                                            .small()
                                            .text_color(cx.theme().warning),
                                    )
                                }),
                        )
                        .child(
                            h_flex()
                                .gap(metrics::gap())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(source.origin.label())
                                .child(inclusion_rule_label(source.rule))
                                .child(format!("scope: {}", source.scope.display()))
                                .when(duplicate_count > 0, |this| {
                                    this.child(format!(
                                        "+{duplicate_count} same-content occurrence(s)"
                                    ))
                                }),
                        ),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    this.selected_context_source = Some(identity.clone());
                    cx.emit(HarnessEvent::OpenFile {
                        path: identity.path.clone(),
                        preview: event.click_count() < 2,
                    });
                    cx.notify();
                }))
        }));

        if let Some(profile) = &context.profile {
            content = content.child(
                v_flex()
                    .gap(metrics::row_gap())
                    .children(field(cx, "Profile", &profile.id))
                    .children(field(cx, "Revision", &profile.source_revision))
                    .children(field(cx, "Retrieved", &profile.retrieved_on))
                    .children(field(cx, "Source", &profile.source_url))
                    .children(field(cx, "Profile SHA-256", &profile.content_digest)),
            );
        }

        if let Some(project_root) = &context.project_root {
            content = content.children(field(
                cx,
                "Project root",
                &project_root.display().to_string(),
            ));
        }

        if !context.assumptions.is_empty() {
            content = content.child(
                v_flex()
                    .gap(metrics::row_gap())
                    .child(context_section_heading(cx, "Assumptions and configuration"))
                    .children(context.assumptions.iter().map(|assumption| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(assumption.clone())
                    })),
            );
        }

        if !context.diagnostics.is_empty() {
            content = content.child(
                v_flex()
                    .gap(metrics::row_gap())
                    .child(context_section_heading(cx, "Diagnostics"))
                    .children(context.diagnostics.iter().map(|diagnostic| {
                        let color = match diagnostic.severity {
                            Severity::Error => cx.theme().danger,
                            Severity::Warning => cx.theme().warning,
                            Severity::Info => cx.theme().muted_foreground,
                        };
                        h_flex()
                            .gap(metrics::gap())
                            .items_start()
                            .text_xs()
                            .when_some(diagnostic.line, |this, line| {
                                this.child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(format!("line {line}")),
                                )
                            })
                            .child(div().flex_1().text_color(color).child(format!(
                                "{}: {}: {}",
                                diagnostic.source, diagnostic.severity, diagnostic.message
                            )))
                    })),
            );
        }

        content = content.child(context_section_heading(cx, "Discovered Skill candidates"));
        if context.available_skills.is_empty() {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No Skill candidates were discovered in the Harness inventory."),
            );
        } else {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Discovered from the all-Harness inventory. Runtime invocation and exact availability are unverified; contents are not part of automatic AGENTS instructions.",
                    ),
            );
            content = content.children(context.available_skills.iter().map(|skill| {
                let entry = skill.entry.clone();
                ListItem::new(format!("harness-context-skill:{}", skill.entry.display()))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_1()
                    .rounded(cx.theme().radius)
                    .child(
                        h_flex()
                            .gap(metrics::gap())
                            .items_center()
                            .child(Icon::new(IconName::Bot).small())
                            .child(div().flex_1().text_sm().child(skill.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(skill.entry.display().to_string()),
                            ),
                    )
                    .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                        cx.emit(HarnessEvent::OpenFile {
                            path: entry.clone(),
                            preview: event.click_count() < 2,
                        });
                    }))
            }));
        }

        content.into_any_element()
    }

    fn render_context_summary(&self, cx: &Context<Self>) -> AnyElement {
        let profile_id = self.profile_select.read(cx).selected_value().cloned();
        let verified = profile_id.as_deref() == Some(CODEX_PROFILE_ID);
        let status = if verified {
            "Verified AGENTS instructions"
        } else {
            "Inventory only · no context resolution"
        };
        let current_codex_home = self.codex_home_input.read(cx).value().to_string();
        let codex_home_source = if Path::new(&current_codex_home) != self.codex_home_default {
            "Edited scenario input"
        } else {
            match self.codex_home_source {
                CodexHomeSource::EnvironmentOverride => {
                    "Explicit CODEX_HOME override; resolver diagnostics determine validity"
                }
                CodexHomeSource::UserHomeDefault => "Defaulted from <home>/.codex",
                CodexHomeSource::HomeUnavailable => {
                    "CODEX_HOME is unset and the platform home directory is unavailable"
                }
            }
        };
        v_flex()
            .id("harness-context-summary")
            .gap(metrics::row_gap())
            .child(
                h_flex()
                    .gap(metrics::gap())
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(if verified {
                                cx.theme().info
                            } else {
                                cx.theme().warning
                            })
                            .child(status),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("harness-context-workspace")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Workspace: {}", self.root.display())),
                    ),
            )
            .children(field(cx, "CODEX_HOME source", codex_home_source))
            .into_any_element()
    }

    fn render_context_inspector(&self, cx: &Context<Self>) -> AnyElement {
        let Some(context) = &self.resolved_context else {
            return v_flex()
                .p(metrics::inset())
                .gap(metrics::gap())
                .child(context_section_heading(cx, "Effective source"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select a resolved chain occurrence to inspect its provenance."),
                )
                .into_any_element();
        };
        let Some(identity) = &self.selected_context_source else {
            return v_flex()
                .p(metrics::inset())
                .gap(metrics::gap())
                .child(context_section_heading(cx, "Effective source"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("This result has no effective instruction occurrences."),
                )
                .into_any_element();
        };
        let Some((index, occurrence)) = context
            .sources
            .iter()
            .enumerate()
            .find(|(_, source)| ContextSourceIdentity::from(*source) == *identity)
        else {
            return div().into_any_element();
        };
        let path = occurrence.path.clone();
        let duplicate_paths = context
            .sources
            .iter()
            .enumerate()
            .filter(|(other_index, source)| {
                *other_index != index
                    && occurrence.content_index.is_some()
                    && source.content_index == occurrence.content_index
            })
            .map(|(other_index, source)| {
                format!("{} · occurrence {}", source.path.display(), other_index + 1)
            })
            .collect::<Vec<_>>();

        v_flex()
            .p(metrics::inset())
            .gap(metrics::gap())
            .child(context_section_heading(cx, "Effective source"))
            .child(
                h_flex()
                    .gap(metrics::gap())
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .child(format!("Occurrence {}", index + 1)),
                    )
                    .child(
                        Button::new("harness-context-open-source")
                            .label("Open source")
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HarnessEvent::OpenFile {
                                    path: path.clone(),
                                    preview: false,
                                });
                            })),
                    ),
            )
            .children(field(cx, "Path", &occurrence.path.display().to_string()))
            .children(field(cx, "Origin", occurrence.origin.label()))
            .children(field(cx, "Rule", inclusion_rule_label(occurrence.rule)))
            .children(field(cx, "Scope", &occurrence.scope.display().to_string()))
            .children(
                occurrence
                    .physical_path
                    .as_ref()
                    .and_then(|path| field(cx, "Physical file", &path.display().to_string())),
            )
            .children(field(cx, "Raw bytes", &occurrence.raw_bytes.to_string()))
            .children(field(
                cx,
                "Included bytes",
                &occurrence.included_bytes.to_string(),
            ))
            .children(field(
                cx,
                "Project bytes before",
                &occurrence.project_bytes_before.to_string(),
            ))
            .children(field(
                cx,
                "Truncated",
                if occurrence.is_truncated() {
                    "Yes"
                } else {
                    "No"
                },
            ))
            .children((!duplicate_paths.is_empty()).then(|| {
                v_flex()
                    .gap(metrics::row_gap())
                    .child(label(cx, "Same effective content also reached through"))
                    .children(duplicate_paths.iter().map(|path| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(path.clone())
                    }))
            }))
            .into_any_element()
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.skills.is_empty() {
            let hint = if self.scanning {
                i18n::t(i18n::Key::Scanning, cx).to_string()
            } else {
                format!(
                    "No skills found.\n\nSearched {} workspace conventions \
                     (skills/, .agents/skills, .claude/skills, …) and {} global \
                     harness directories.",
                    skill::discovery_roots().len(),
                    mt_core::agent_artifacts::harness::global_roots().len(),
                )
            };
            return v_flex()
                .p(metrics::inset())
                .gap(metrics::gap())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
                .into_any_element();
        }

        let group_by = AppSettings::global(cx).skills_group_by;
        let rows = group(&self.skills, group_by);

        v_flex()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .children(rows.into_iter().map(|Row { ix, heading }| {
                let skill = &self.skills[ix];
                let selected = self.selected == Some(ix);
                let invalid = !skill.is_valid();
                v_flex()
                    .gap_0p5()
                    .children(heading.map(|heading| {
                        div()
                            .px(metrics::row_pad())
                            .pt_2()
                            .pb_0p5()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().muted_foreground)
                            .child(heading)
                    }))
                    .child(
                        ListItem::new(ix)
                            .w_full()
                            .px(metrics::row_pad())
                            .py_1()
                            .rounded(cx.theme().radius)
                            .selected(selected)
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(Icon::new(IconName::Bot).small())
                                            .child(div().text_sm().child(skill.name.clone()))
                                            .when(!skill.aliases.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!(
                                                            "+{} link(s)",
                                                            skill.aliases.len()
                                                        )),
                                                )
                                            })
                                            .when(invalid, |this| {
                                                this.child(
                                                    Icon::new(IconName::TriangleAlert)
                                                        .small()
                                                        .text_color(cx.theme().danger),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(skill.summary().to_string()),
                                    ),
                            )
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.selected = Some(ix);
                                // A single click selects and previews; a second
                                // promotes the tab. Same rule the file tree and
                                // VS Code use, so the two lists behave alike.
                                if let Some(path) = this.entry_path(ix) {
                                    cx.emit(HarnessEvent::OpenFile {
                                        path,
                                        preview: event.click_count() < 2,
                                    });
                                }
                                cx.notify();
                            })),
                    )
            }))
            .into_any_element()
    }

    /// The details of whatever is selected, rendered wherever the caller puts
    /// it.
    ///
    /// Public because it no longer lives under the list: cramming metadata,
    /// file lists and validation output into the bottom of a 268px column left
    /// both halves too short to read. The workspace hosts this in the right
    /// panel instead.
    pub fn render_details(&self, cx: &Context<Self>) -> AnyElement {
        match self.section {
            Section::Skills => self.render_inspector(cx),
            Section::Instructions => self.render_instruction_inspector(cx),
            Section::Context => self.render_context_inspector(cx),
        }
    }

    /// Whether anything is selected, so the caller can skip an empty panel.
    pub fn has_selection(&self) -> bool {
        self.selected_skill().is_some()
            || self.selected_instruction().is_some()
            || self.selected_context_source.is_some()
    }

    fn render_inspector(&self, cx: &Context<Self>) -> AnyElement {
        let Some(skill) = self.selected_skill() else {
            return div().into_any_element();
        };

        let entry = skill.entry.clone();
        v_flex()
            .p(metrics::inset())
            .gap(metrics::gap())
            .child(
                h_flex()
                    .gap(metrics::gap())
                    .items_center()
                    .child(div().text_sm().font_semibold().child(skill.name.clone()))
                    .child(div().flex_1())
                    .child(
                        Button::new("open-skill")
                            .label(i18n::t(i18n::Key::OpenSkillMd, cx))
                            .xsmall()
                            .primary()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HarnessEvent::OpenFile {
                                    path: entry.clone(),
                                    preview: false,
                                });
                            })),
                    ),
            )
            .child(div().text_xs().child(skill.summary().to_string()))
            .children(field(
                cx,
                i18n::t(i18n::Key::Origin, cx),
                skill.origin.label(),
            ))
            .children(field(
                cx,
                i18n::t(i18n::Key::Location, cx),
                &located(&self.root, skill.origin, &skill.dir),
            ))
            .children(field(
                cx,
                i18n::t(i18n::Key::DiscoveredIn, cx),
                &located(&self.root, skill.origin, &skill.root),
            ))
            // The same skill reached by several paths — typically a harness
            // directory symlinked or junctioned into a canonical one. Showing
            // the links is what makes the deduplication legible rather than
            // looking like a missing entry.
            .children((!skill.aliases.is_empty()).then(|| {
                v_flex()
                    .gap_0p5()
                    .child(label(cx, "Also linked from"))
                    .children(skill.aliases.iter().map(|alias| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(located(&self.root, skill.origin, alias))
                    }))
            }))
            .children(
                skill
                    .meta
                    .license
                    .as_deref()
                    .and_then(|v| field(cx, "License", v)),
            )
            .children(
                (!skill.meta.allowed_tools.is_empty())
                    .then(|| field(cx, "Allowed tools", &skill.meta.allowed_tools.join(" ")))
                    .flatten(),
            )
            .children(
                skill
                    .meta
                    .compatibility
                    .as_deref()
                    .and_then(|v| field(cx, "Compatibility", v)),
            )
            .children(
                skill
                    .meta
                    .metadata
                    .iter()
                    .filter_map(|(k, v)| field(cx, k, v)),
            )
            .children(
                skill
                    .meta
                    .extra
                    .iter()
                    .filter_map(|(k, v)| field(cx, &format!("{k} (non-standard)"), v)),
            )
            // Supporting directories: `scripts/`, `references/`, `assets/`.
            .children((!skill.support_dirs.is_empty()).then(|| {
                v_flex()
                    .gap_0p5()
                    .mt_1()
                    .child(label(cx, i18n::t(i18n::Key::Files, cx)))
                    .children(
                        std::iter::once(
                            div()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child("SKILL.md")
                                .into_any_element(),
                        )
                        .chain(skill.support_dirs.iter().map(|dir| {
                            div()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(format!(
                                    "{}/",
                                    dir.file_name().and_then(|n| n.to_str()).unwrap_or_default()
                                ))
                                .into_any_element()
                        })),
                    )
            }))
            // Validation results last: they are the reason to open this panel
            // when something is wrong, but noise when everything is fine.
            .children((!skill.diagnostics.is_empty()).then(|| {
                v_flex()
                    .gap_0p5()
                    .mt_1()
                    .child(label(cx, i18n::t(i18n::Key::Validation, cx)))
                    .children(skill.diagnostics.iter().map(|d| {
                        let color = match d.severity {
                            Severity::Error => cx.theme().danger,
                            Severity::Warning => cx.theme().warning,
                            Severity::Info => cx.theme().muted_foreground,
                        };
                        h_flex()
                            .gap_2()
                            .items_start()
                            .text_xs()
                            // The line is what turns "this field is wrong" into
                            // something the reader can act on without scanning
                            // the file for the field the message names.
                            .when_some(d.line, |this, line| {
                                this.child(
                                    div()
                                        .w(px(48.))
                                        .flex_shrink_0()
                                        .text_color(cx.theme().muted_foreground)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(format!("line {line}")),
                                )
                            })
                            .child(div().flex_1().text_color(color).child(d.message.clone()))
                    }))
            }))
            .into_any_element()
    }
}

/// One list row: a skill, and the group heading that precedes it (if it is the
/// first of its group).
struct Row {
    ix: usize,
    heading: Option<String>,
}

fn artifacts_under(skills: &[Skill], instructions: &[Instruction], path: &Path) -> bool {
    skills.iter().any(|skill| {
        skill.dir.starts_with(path)
            || skill.aliases.iter().any(|alias| alias.starts_with(path))
            || skill
                .support_dirs
                .iter()
                .any(|directory| directory.starts_with(path))
    }) || instructions.iter().any(|instruction| {
        instruction.path.starts_with(path)
            || instruction
                .aliases
                .iter()
                .any(|alias| alias.starts_with(path))
    })
}

/// Order `skills` for display and decide where headings fall.
///
/// A pure function over indices rather than a method: grouping is the part with
/// rules worth testing, and testing it should not need a window.
fn group(skills: &[Skill], group_by: GroupBy) -> Vec<Row> {
    let key = |skill: &Skill| -> Option<String> {
        match group_by {
            GroupBy::None => None,
            GroupBy::Origin => Some(skill.origin.label().to_uppercase()),
            GroupBy::Harness => Some(
                mt_core::agent_artifacts::harness::label_for_root(
                    &skill.root,
                    skill.origin == Origin::Global,
                )
                .to_uppercase(),
            ),
            GroupBy::Status => Some(
                if skill.is_valid() {
                    "VALID"
                } else {
                    "NEEDS ATTENTION"
                }
                .to_string(),
            ),
        }
    };

    let mut order: Vec<usize> = (0..skills.len()).collect();
    // Stable, so the discovery order (origin, then name) still decides within a
    // group. `Status` puts the problems first: a conformance sweep is the
    // reason to group by it at all.
    order.sort_by_key(|&ix| match group_by {
        GroupBy::Status => (
            skills[ix].is_valid() as u8,
            key(&skills[ix]).unwrap_or_default(),
        ),
        _ => (0, key(&skills[ix]).unwrap_or_default()),
    });

    let mut rows = Vec::with_capacity(order.len());
    let mut previous: Option<String> = None;
    for ix in order {
        let heading = key(&skills[ix]);
        let show = heading.is_some() && heading != previous;
        rows.push(Row {
            ix,
            heading: show.then(|| heading.clone().unwrap_or_default()),
        });
        previous = heading;
    }
    rows
}

fn default_codex_home() -> (PathBuf, CodexHomeSource) {
    codex_home_default(std::env::var_os("CODEX_HOME"), dirs::home_dir())
}

fn target_parent_as_cwd(target: &Path) -> Option<PathBuf> {
    target.parent().map(Path::to_path_buf)
}

fn codex_home_default(
    codex_home: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> (PathBuf, CodexHomeSource) {
    if let Some(codex_home) = codex_home.filter(|value| !value.is_empty()) {
        return (
            PathBuf::from(codex_home),
            CodexHomeSource::EnvironmentOverride,
        );
    }
    if let Some(home) = home {
        return (home.join(".codex"), CodexHomeSource::UserHomeDefault);
    }
    (PathBuf::new(), CodexHomeSource::HomeUnavailable)
}

fn codex_home_is_override(source: CodexHomeSource, default: &Path, current: &Path) -> bool {
    source == CodexHomeSource::EnvironmentOverride || current != default
}

fn split_config_names(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value.split(',').map(str::to_string).collect()
    }
}

fn context_matches_input(input: Option<&ContextInput>, context: &ResolvedContext) -> bool {
    context_input_matches(input, &context.input)
}

fn context_input_matches(input: Option<&ContextInput>, resolved_input: &ContextInput) -> bool {
    input == Some(resolved_input)
}

fn context_source_element_id(source: &SourceOccurrence) -> String {
    format!(
        "harness-context-source:{:?}:{}:{}",
        source.rule,
        source.scope.display(),
        source.path.display()
    )
}

fn project_trust_label(trust: ProjectTrust) -> &'static str {
    match trust {
        ProjectTrust::Trusted => "Trusted",
        ProjectTrust::Untrusted => "Untrusted",
        ProjectTrust::Unspecified => "Unspecified",
    }
}

fn resolution_status_label(status: ResolutionStatus) -> &'static str {
    match status {
        ResolutionStatus::Resolved => "Resolved",
        ResolutionStatus::Partial => "Partial",
        ResolutionStatus::InvalidInput => "Invalid input",
        ResolutionStatus::InventoryOnly => "Inventory only",
    }
}

fn inclusion_rule_label(rule: InclusionRule) -> &'static str {
    match rule {
        InclusionRule::GlobalOverride => "global override",
        InclusionRule::GlobalAgents => "global AGENTS.md",
        InclusionRule::ProjectOverride => "project override",
        InclusionRule::ProjectAgents => "project AGENTS.md",
        InclusionRule::ProjectFallback => "project fallback",
    }
}

fn context_profile_choices() -> Vec<ContextProfileChoice> {
    let mut choices = vec![ContextProfileChoice {
        profile_id: CODEX_PROFILE_ID.to_string(),
        supported: true,
    }];
    for harness in mt_core::agent_artifacts::harness::HARNESSES {
        let profile_id = harness.id.to_string();
        if !choices.iter().any(|choice| choice.profile_id == profile_id) {
            choices.push(ContextProfileChoice {
                profile_id,
                supported: false,
            });
        }
    }
    choices
}

fn context_profile_selector(
    state: &Entity<SelectState<SearchableVec<ContextProfileChoice>>>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_0p5()
        .child(label(cx, "Harness profile"))
        .child(
            Select::new(state)
                .id("harness-context-profile")
                .placeholder("Choose a harness profile")
                .accessibility_label("Harness profile")
                .search_placeholder("Search profiles"),
        )
        .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Only codex-agents-md-2026-08-29 resolves automatic AGENTS instructions; other Harness entries are inventory-only.",
                    ),
        )
        .into_any_element()
}

fn context_input_field(
    input: &Entity<InputState>,
    name: &str,
    id: &str,
    help: &str,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(label(cx, name))
        .child(
            Input::new(input)
                .id(SharedString::from(id.to_string()))
                .accessibility_id(id)
                .aria_label(name),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(help.to_string()),
        )
}

fn context_section_heading(cx: &App, text: &str) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

fn field(cx: &App, name: &str, value: &str) -> Option<AnyElement> {
    if value.is_empty() {
        return None;
    }
    Some(
        h_flex()
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
            .child(div().flex_1().child(value.to_string()))
            .into_any_element(),
    )
}

fn label(cx: &App, text: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

/// Display a path the way its origin makes readable.
///
/// A workspace skill reads best relative to the workspace; a global one is not
/// under it at all, so relativizing would produce a wall of `../..`. Abbreviate
/// the home prefix instead, which is how these paths are written everywhere
/// else.
fn located(root: &Path, origin: Origin, path: &Path) -> String {
    match origin {
        Origin::Workspace => mt_core::workspace::display_relative(root, path),
        Origin::Global => abbreviate_home(path),
    }
}

fn abbreviate_home(path: &Path) -> String {
    let home = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.trim().is_empty());
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.to_string_lossy().replace('\\', "/"));
    }
    path.to_string_lossy().replace('\\', "/")
}

impl EventEmitter<HarnessEvent> for HarnessView {}

impl Focusable for HarnessView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HarnessView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let skills = self.skills.len();
        let invalid = self.skills.iter().filter(|s| !s.is_valid()).count();
        let group_by = AppSettings::global(cx).skills_group_by;
        let section = self.section;

        v_flex()
            .id("harness")
            .role(gpui_kit::Role::List)
            .aria_label("Harness artifacts")
            .track_focus(&self.focus_handle)
            .size_full()
            // Skills and instruction files are both harness artifacts, but they
            // have nothing in common row-for-row — one has a schema to validate,
            // the other is prose. Two sections rather than one merged list.
            .child(
                TabBar::new("harness-sections")
                    .segmented()
                    .w_full()
                    .px(metrics::inset())
                    .py(metrics::header_pad_y())
                    .selected_index(Section::ALL.iter().position(|s| *s == section).unwrap_or(0))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.set_section(Section::ALL[*ix], cx);
                    }))
                    .children(Section::ALL.map(|s| {
                        Tab::new().label(
                            s.label()
                                .map(|key| i18n::t(key, cx).to_string())
                                .unwrap_or_else(|| "Context".to_string()),
                        )
                    })),
            )
            .child(
                h_flex()
                    .px(metrics::inset())
                    .py(metrics::header_pad_y())
                    .gap(metrics::gap())
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().muted_foreground)
                            .child(match section {
                                Section::Skills => format!("SKILLS ({skills})"),
                                Section::Instructions => {
                                    format!("INSTRUCTIONS ({})", self.instructions.len())
                                }
                                Section::Context => format!(
                                    "EFFECTIVE CONTEXT ({})",
                                    self.resolved_context
                                        .as_ref()
                                        .map_or(0, |context| context.sources.len())
                                ),
                            }),
                    )
                    .when(section == Section::Skills && invalid > 0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(format!("{invalid} invalid")),
                        )
                    })
                    .child(
                        Button::new("rescan")
                            // Not `Redo`: a single curved arrow is the universal
                            // "undo/revert" glyph, and on a button that rescans
                            // the filesystem it reads as though it will put
                            // something back. `refresh-cw` is a closed cycle.
                            .icon(Icon::empty().path("icons/refresh-cw.svg"))
                            .xsmall()
                            .ghost()
                            // Swaps the icon for a spinner and makes the button
                            // inert, which is also what stops a second click
                            // from queueing a redundant scan mid-flight.
                            .loading(self.scanning)
                            .tooltip(i18n::t(i18n::Key::Rescan, cx))
                            .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                    ),
            )
            // Grouping is a view choice, so it belongs next to the list rather
            // than buried in settings — but it persists there, because a user
            // who groups by harness means it next time too. Instruction files
            // are a flat handful, so it only applies to skills.
            .when(section == Section::Skills, |this| {
                this.child(
                    h_flex()
                        .px(metrics::inset())
                        .pb(metrics::header_pad_y())
                        .gap(metrics::gap())
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(i18n::t(i18n::Key::GroupBy, cx)),
                        )
                        .children(GroupBy::ALL.map(|option| {
                            Button::new(SharedString::from(format!("group-{}", option.key())))
                                .label(option.label())
                                .xsmall()
                                .when(option == group_by, |b| b.primary())
                                .when(option != group_by, |b| b.ghost())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    AppSettings::update(cx, |settings| {
                                        settings.skills_group_by = option
                                    });
                                    // Grouping only reorders what is already
                                    // loaded; rescanning the filesystem for a
                                    // view change would be gratuitous.
                                    this.keep_selection_stable(cx);
                                }))
                        })),
                )
            })
            .child(
                div()
                    .id("harness-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .map(|this| match section {
                        Section::Skills => this.child(self.render_list(cx)),
                        Section::Instructions => this.child(self.render_instructions(cx)),
                        Section::Context => this.child(self.render_context_panel(cx)),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui_kit::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{
        CodexHomeSource, ContextSourceIdentity, HarnessView, Row, Section, artifacts_under,
        codex_home_default, codex_home_is_override, context_input_matches, context_profile_choices,
        context_source_element_id, group, populated_section, split_config_names,
        target_parent_as_cwd,
    };
    use mt_core::agent_artifacts::context::{
        ContextContent, ContextInput, ContextProfile, InclusionRule, ProjectTrust,
        ResolutionStatus, ResolvedContext, SourceOccurrence,
    };
    use mt_core::agent_artifacts::skill::{Skill, SkillMeta};
    use mt_core::settings::GroupBy;
    use mt_core::{Diagnostic, DocType, Instruction, Origin};
    use std::path::{Path, PathBuf};

    #[test]
    fn both_sections_are_reachable_and_named_distinctly() {
        // The panel is one surface over two artifact kinds; a section that
        // cannot be selected, or that shares a label, is a section that does
        // not exist as far as the user is concerned.
        use crate::i18n::{Key, text};
        use mt_core::settings::Language;

        let keys: Vec<Key> = [Section::Skills, Section::Instructions]
            .into_iter()
            .filter_map(Section::label)
            .collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(Section::ALL.len(), 3);
        assert!(Section::ALL.contains(&Section::Context));
        assert_ne!(keys[0], keys[1], "the two sections share a label key");
        // …and the strings behind them differ in every language, which is what
        // the user actually sees.
        for language in Language::ALL {
            assert_ne!(
                text(keys[0], language),
                text(keys[1], language),
                "{}",
                language.label()
            );
        }
    }

    #[test]
    fn discovery_keeps_a_populated_section_and_falls_back_from_an_empty_one() {
        assert_eq!(
            populated_section(Section::Skills, 1, 1),
            Some(Section::Skills)
        );
        assert_eq!(
            populated_section(Section::Instructions, 1, 1),
            Some(Section::Instructions)
        );
        assert_eq!(
            populated_section(Section::Skills, 0, 1),
            Some(Section::Instructions),
            "an instructions-only workspace must make its details reachable"
        );
        assert_eq!(
            populated_section(Section::Instructions, 1, 0),
            Some(Section::Skills),
            "a skills-only workspace must make its details reachable"
        );
        assert_eq!(populated_section(Section::Skills, 0, 0), None);
    }

    fn skill(name: &str, origin: Origin, root: &str, valid: bool) -> Skill {
        Skill {
            dir: PathBuf::from(root).join(name),
            entry: PathBuf::from(root).join(name).join("SKILL.md"),
            root: PathBuf::from(root),
            origin,
            aliases: Vec::new(),
            name: name.to_string(),
            meta: SkillMeta::default(),
            diagnostics: if valid {
                Vec::new()
            } else {
                vec![Diagnostic::error("skill", "broken")]
            },
            support_dirs: Vec::new(),
        }
    }

    #[test]
    fn removed_dotted_directories_are_matched_against_current_artifacts() {
        let mut review = skill("review.v2", Origin::Workspace, "skills", true);
        review.support_dirs.push(review.dir.join("references"));
        let instruction = Instruction {
            path: PathBuf::from(".cursor/rules/team.v2/AGENT.md"),
            root: PathBuf::from(".cursor"),
            harness_dir: ".cursor".into(),
            origin: Origin::Workspace,
            doc_type: DocType::Agents,
            aliases: Vec::new(),
        };

        assert!(artifacts_under(
            std::slice::from_ref(&review),
            std::slice::from_ref(&instruction),
            PathBuf::from("skills/review.v2").as_path(),
        ));
        assert!(artifacts_under(
            std::slice::from_ref(&review),
            std::slice::from_ref(&instruction),
            PathBuf::from(".cursor/rules/team.v2").as_path(),
        ));
        assert!(!artifacts_under(
            std::slice::from_ref(&review),
            std::slice::from_ref(&instruction),
            PathBuf::from("skills/other").as_path(),
        ));
    }

    fn headings(rows: &[Row]) -> Vec<&str> {
        rows.iter().filter_map(|r| r.heading.as_deref()).collect()
    }

    #[test]
    fn no_grouping_lists_everything_once_with_no_headings() {
        let skills = vec![
            skill("a", Origin::Workspace, "/w/.claude/skills", true),
            skill("b", Origin::Global, "/h/.agents/skills", true),
        ];
        let rows = group(&skills, GroupBy::None);
        assert_eq!(rows.len(), 2);
        assert!(headings(&rows).is_empty());
    }

    #[test]
    fn origin_grouping_emits_one_heading_per_origin() {
        let skills = vec![
            skill("a", Origin::Workspace, "/w/.claude/skills", true),
            skill("b", Origin::Workspace, "/w/.claude/skills", true),
            skill("c", Origin::Global, "/h/.agents/skills", true),
        ];
        let rows = group(&skills, GroupBy::Origin);
        assert_eq!(headings(&rows), vec!["GLOBAL", "WORKSPACE"]);
        assert_eq!(rows.len(), 3, "every skill still appears");
    }

    #[test]
    fn harness_grouping_uses_the_root_the_skill_was_found_under() {
        let skills = vec![
            skill("a", Origin::Workspace, "/w/.factory/skills", true),
            skill("b", Origin::Workspace, "/w/.goose/skills", true),
        ];
        let rows = group(&skills, GroupBy::Harness);
        assert_eq!(headings(&rows), vec!["DROID", "GOOSE"]);
    }

    #[test]
    fn status_grouping_puts_the_problems_first() {
        // The reason to group by status is to find what needs fixing; burying
        // it below a hundred valid skills would defeat that.
        let skills = vec![
            skill("ok", Origin::Workspace, "/w/skills", true),
            skill("broken", Origin::Workspace, "/w/skills", false),
        ];
        let rows = group(&skills, GroupBy::Status);
        assert_eq!(headings(&rows), vec!["NEEDS ATTENTION", "VALID"]);
        assert_eq!(rows[0].ix, 1, "the invalid skill leads");
    }

    #[test]
    fn every_grouping_shows_every_skill_exactly_once() {
        // A grouping that drops or duplicates a row is the bug worth guarding:
        // it looks like a discovery failure.
        let skills = vec![
            skill("a", Origin::Workspace, "/w/skills", true),
            skill("b", Origin::Global, "/h/.agents/skills", false),
            skill("c", Origin::Global, "/h/.claude/skills", true),
        ];
        for option in GroupBy::ALL {
            let rows = group(&skills, option);
            let mut seen: Vec<usize> = rows.iter().map(|r| r.ix).collect();
            seen.sort_unstable();
            assert_eq!(
                seen,
                vec![0, 1, 2],
                "{} lost or duplicated a skill",
                option.label()
            );
        }
    }

    #[test]
    fn an_empty_list_groups_to_nothing() {
        for option in GroupBy::ALL {
            assert!(group(&[], option).is_empty(), "{}", option.label());
        }
    }

    #[test]
    fn the_spinner_floor_is_long_enough_to_perceive_and_short_enough_to_ignore() {
        use super::SPINNER_FLOOR;
        use std::time::Duration;

        assert!(
            SPINNER_FLOOR >= Duration::from_millis(150),
            "below ~150ms a state change is not reliably perceived"
        );
        assert!(
            SPINNER_FLOOR <= Duration::from_millis(500),
            "a floor long enough to notice as a delay would make rescan feel slow"
        );
    }

    fn context_input() -> ContextInput {
        ContextInput {
            workspace: PathBuf::from("/workspace"),
            target: PathBuf::from("/workspace/task.md"),
            cwd: PathBuf::from("/workspace"),
            profile_id: super::CODEX_PROFILE_ID.to_string(),
            codex_home: PathBuf::from("/home/user/.codex"),
            codex_home_override: false,
            fallback_filenames: Vec::new(),
            project_doc_max_bytes: 32_768,
            project_root_markers: vec![".git".to_string()],
            project_trust: ProjectTrust::Unspecified,
        }
    }

    #[test]
    fn stale_resolution_identity_rejects_target_and_configuration_changes() {
        let input = context_input();
        assert!(context_input_matches(Some(&input), &input));
        assert!(!context_input_matches(None, &input));

        let mut changed_target = input.clone();
        changed_target.target = PathBuf::from("/workspace/other.md");
        assert!(!context_input_matches(Some(&input), &changed_target));

        let mut changed_cwd = input.clone();
        changed_cwd.cwd = PathBuf::from("/workspace/nested");
        assert!(!context_input_matches(Some(&input), &changed_cwd));

        let mut changed_config = input.clone();
        changed_config.project_doc_max_bytes = 16_384;
        assert!(!context_input_matches(Some(&input), &changed_config));
    }

    #[test]
    fn context_source_element_identity_tracks_path_and_provenance() {
        let source = SourceOccurrence {
            path: PathBuf::from("/workspace/AGENTS.md"),
            physical_path: None,
            origin: Origin::Workspace,
            scope: PathBuf::from("/workspace"),
            rule: InclusionRule::ProjectAgents,
            raw_bytes: 8,
            included_bytes: 8,
            project_bytes_before: 0,
            content_index: Some(0),
        };
        let same_identity = SourceOccurrence {
            path: source.path.clone(),
            ..source.clone()
        };
        let alias = SourceOccurrence {
            path: PathBuf::from("/workspace/nested/AGENTS.md"),
            scope: PathBuf::from("/workspace/nested"),
            ..source.clone()
        };

        assert_eq!(
            ContextSourceIdentity::from(&source),
            ContextSourceIdentity::from(&same_identity)
        );
        assert_eq!(
            context_source_element_id(&source),
            context_source_element_id(&same_identity)
        );
        assert_ne!(
            context_source_element_id(&source),
            context_source_element_id(&alias)
        );
    }

    #[test]
    fn context_name_inputs_keep_order_and_leave_normalization_to_the_profile() {
        assert!(split_config_names("").is_empty());
        assert_eq!(
            split_config_names("AGENTS.md, TEAM.md,,"),
            vec!["AGENTS.md", " TEAM.md", "", ""]
        );
    }

    #[test]
    fn profile_selector_supports_only_the_frozen_snapshot() {
        let choices = context_profile_choices();
        assert_eq!(
            choices.first().map(|choice| choice.profile_id.as_str()),
            Some(super::CODEX_PROFILE_ID)
        );
        assert!(choices[0].supported);
        assert!(choices[1..].iter().all(|choice| !choice.supported));
        assert!(
            choices
                .iter()
                .any(|choice| choice.profile_id == "codex" && !choice.supported)
        );
        let mut ids: Vec<_> = choices
            .iter()
            .map(|choice| choice.profile_id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), choices.len());
    }

    #[test]
    fn explicit_codex_home_is_preserved_exactly_instead_of_using_home_default() {
        let override_value = std::ffi::OsString::from("  invalid-codex-home  ");
        let (path, source) = codex_home_default(
            Some(override_value.clone()),
            Some(PathBuf::from("/home/user")),
        );

        assert_eq!(path, PathBuf::from(override_value));
        assert_eq!(source, CodexHomeSource::EnvironmentOverride);
    }

    #[test]
    fn codex_home_default_distinguishes_missing_environment_and_missing_home() {
        assert_eq!(
            codex_home_default(None, Some(PathBuf::from("/home/user"))),
            (
                PathBuf::from("/home/user/.codex"),
                CodexHomeSource::UserHomeDefault
            )
        );
        assert_eq!(
            codex_home_default(None, None),
            (PathBuf::new(), CodexHomeSource::HomeUnavailable)
        );
        assert_eq!(
            codex_home_default(
                Some(std::ffi::OsString::new()),
                Some(PathBuf::from("/home/user"))
            ),
            (
                PathBuf::from("/home/user/.codex"),
                CodexHomeSource::UserHomeDefault
            )
        );
    }

    #[test]
    fn codex_home_edit_is_explicit_and_environment_override_provenance_is_retained() {
        let default = PathBuf::from("/home/user/.codex");
        assert!(!codex_home_is_override(
            CodexHomeSource::UserHomeDefault,
            &default,
            &default
        ));
        assert!(codex_home_is_override(
            CodexHomeSource::UserHomeDefault,
            &default,
            Path::new("/tmp/custom-codex")
        ));
        assert!(codex_home_is_override(
            CodexHomeSource::EnvironmentOverride,
            &default,
            &default
        ));
    }

    #[test]
    fn target_parent_cwd_action_uses_the_lexical_file_parent() {
        let target = PathBuf::from("workspace").join("src").join("auth.rs");
        assert_eq!(
            target_parent_as_cwd(&target),
            Some(PathBuf::from("workspace").join("src"))
        );
        assert_eq!(
            target,
            PathBuf::from("workspace").join("src").join("auth.rs")
        );
    }

    #[test]
    fn remote_scenario_inputs_never_acquire_context_watches() {
        let mut input = context_input();
        input.codex_home = PathBuf::from("\\\\context-test.invalid\\share\0");
        let context = mt_core::agent_artifacts::context::resolve(&input, &[]);
        assert_eq!(context.status, ResolutionStatus::InvalidInput);
        assert!(
            context
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.source == "context.unsupported-remote" })
        );
        assert!(context.watch_paths().is_empty());
        assert!(context.watch_directories().is_empty());
    }

    #[gpui_kit::test]
    fn context_source_navigation_and_target_parent_action_keep_paths_independent(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::sync::{Arc, Mutex};

        let directory = tempfile::tempdir().expect("workspace directory");
        let root = directory.path().to_path_buf();
        cx.update(|app| {
            gpui_kit::init(app);
            crate::settings::AppSettings::init(app);
        });
        let root_for_view = root.clone();
        let skill_cache = Arc::new(Mutex::new(
            mt_core::agent_artifacts::skill::DiscoveryCache::default(),
        ));
        let (view, cx) = cx.add_window_view(move |window, cx| {
            HarnessView::new(root_for_view, skill_cache, window, cx)
        });
        let input = view.read_with(cx, |view, _| {
            view.context_input().expect("initial input").clone()
        });
        let source = SourceOccurrence {
            path: root.join("AGENTS.md"),
            physical_path: None,
            origin: Origin::Workspace,
            scope: root.clone(),
            rule: InclusionRule::ProjectAgents,
            raw_bytes: 8,
            included_bytes: 8,
            project_bytes_before: 0,
            content_index: Some(0),
        };
        let resolved = ResolvedContext {
            input: input.clone(),
            profile: Some(ContextProfile::codex()),
            project_root: Some(root.clone()),
            status: ResolutionStatus::Resolved,
            assumptions: Vec::new(),
            discovery_digest: None,
            sources: vec![source.clone()],
            contents: vec![ContextContent {
                text: "context fixture".to_string(),
            }],
            diagnostics: Vec::new(),
            available_skills: Vec::new(),
        };
        view.update(cx, |view, cx| {
            view.set_section(Section::Context, cx);
            assert!(view.apply_resolved_context(resolved, cx));
        });

        let source_id = context_source_element_id(&source);
        let target = root.join("src/auth.rs");
        let target_text = target.to_string_lossy().to_string();
        cx.update(|window, app| {
            window.render_frame(app);
            window.scroll(
                "harness-list",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-2_000.),
                )),
                app,
            );
            window.render_frame(app);
            window.click(gpui_kit::SharedString::from(source_id.clone()), app);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, app| {
            assert_eq!(view.selected_target(app), root);
            assert!(view.resolved_context().is_some());
        });

        cx.update(|window, app| {
            window.scroll(
                "harness-list",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(2_000.),
                )),
                app,
            );
            window.render_frame(app);
            window.click("harness-context-target", app);
            window.press("ctrl-a", app);
            window.input(&target_text, app);
            window.render_frame(app);
            window.click("harness-context-use-target-parent-as-cwd", app);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, app| {
            assert_eq!(view.selected_target(app), target);
            assert_eq!(
                view.context_input().expect("updated input").cwd,
                target.parent().expect("file target parent")
            );
            assert!(view.resolved_context().is_none());
        });
    }
}
