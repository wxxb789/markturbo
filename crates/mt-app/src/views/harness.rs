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

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    tab::{Tab, TabBar},
    v_flex,
};
use mt_doc::{Instruction, Origin, Severity, Skill, instruction, skill};

use crate::i18n;
use crate::metrics;
use crate::settings::{AppSettings, GroupBy};

/// Emitted when the user wants to open an artifact's document.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    /// Open `path`. `preview` means a single click: the tab is transient and
    /// the next single click replaces it, which is what keeps browsing a list
    /// from leaving a bar full of tabs.
    OpenFile { path: PathBuf, preview: bool },
}

/// Which kind of artifact the panel is listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Skills,
    Instructions,
}

impl Section {
    const ALL: [Section; 2] = [Section::Skills, Section::Instructions];

    fn label(self) -> crate::i18n::Key {
        match self {
            Section::Skills => crate::i18n::Key::SectionSkills,
            Section::Instructions => crate::i18n::Key::SectionInstructions,
        }
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
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root,
            skill_cache,
            section: Section::Skills,
            skills: Vec::new(),
            instructions: Vec::new(),
            selected: None,
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
        let options = mt_doc::Discovery {
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
        self.selected = previous
            .and_then(|path| self.position_of(&path))
            .or_else(|| (!self.is_empty()).then_some(0));
        cx.notify();
    }

    /// The path identifying the current selection, whichever section is showing.
    fn selected_path(&self) -> Option<PathBuf> {
        let ix = self.selected?;
        match self.section {
            Section::Skills => self.skills.get(ix).map(|s| s.dir.clone()),
            Section::Instructions => self.instructions.get(ix).map(|i| i.path.clone()),
        }
    }

    fn position_of(&self, path: &Path) -> Option<usize> {
        match self.section {
            Section::Skills => self.skills.iter().position(|s| s.dir == path),
            Section::Instructions => self.instructions.iter().position(|i| i.path == path),
        }
    }

    fn is_empty(&self) -> bool {
        match self.section {
            Section::Skills => self.skills.is_empty(),
            Section::Instructions => self.instructions.is_empty(),
        }
    }

    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
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
                    mt_doc::harness::global_roots().len(),
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
        }
    }

    /// Whether anything is selected, so the caller can skip an empty panel.
    pub fn has_selection(&self) -> bool {
        self.selected_skill().is_some() || self.selected_instruction().is_some()
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
                mt_doc::harness::label_for_root(&skill.root, skill.origin == Origin::Global)
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
                    .w(px(96.))
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
        Origin::Workspace => crate::workspace::display_relative(root, path),
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
            .role(gpui::Role::List)
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
                    .children(Section::ALL.map(|s| Tab::new().label(i18n::t(s.label(), cx)))),
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
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .map(|this| match section {
                        Section::Skills => this.child(self.render_list(cx)),
                        Section::Instructions => this.child(self.render_instructions(cx)),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{Row, Section, artifacts_under, group};
    use crate::settings::GroupBy;
    use mt_doc::skill::{Skill, SkillMeta};
    use mt_doc::{Diagnostic, DocType, Instruction, Origin};
    use std::path::PathBuf;

    #[test]
    fn both_sections_are_reachable_and_named_distinctly() {
        // The panel is one surface over two artifact kinds; a section that
        // cannot be selected, or that shares a label, is a section that does
        // not exist as far as the user is concerned.
        use crate::i18n::{Key, text};
        use crate::settings::Language;

        let keys: Vec<Key> = Section::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(keys.len(), 2);
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

    /// Switching sections must reset the selection.
    ///
    /// A source-level check rather than a runtime one: `selected` is a bare
    /// index into whichever list is showing, so carrying it across a section
    /// change points the inspector at an unrelated artifact — or, when the
    /// other list is shorter, at nothing while the row still looks selected.
    /// The bug is invisible until someone selects the eighth skill and switches
    /// to a panel with three instruction files, which is not a state a unit
    /// test reaches without a window.
    #[test]
    fn changing_section_resets_the_selection() {
        // `include_str!` resolves relative to this file at compile time, so it
        // works regardless of the test runner's working directory.
        let source = crate::views::production_source(include_str!("harness.rs"));
        let body = source
            .split_once("fn set_section")
            .expect("set_section must exist")
            .1;
        let body = body.split("\n    pub fn ").next().unwrap_or(body);
        assert!(
            body.contains("self.selected ="),
            "set_section must reassign the selection; it indexes a list that \
             just changed underneath it"
        );
    }

    /// The rescan button must spin while a scan is in flight.
    ///
    /// Source-level because the assertion is about a rendered `Button`, and
    /// building one needs a `Window`. The failure it guards is the reported
    /// one: with no visible state change a click is indistinguishable from a
    /// hung app, and the user clicks again.
    #[test]
    fn the_rescan_button_reflects_the_scanning_flag() {
        let source = crate::views::production_source(include_str!("harness.rs"));
        let button = source
            .split_once("Button::new(\"rescan\")")
            .expect("the rescan button must exist")
            .1;
        let button = button.split("on_click").next().unwrap_or(button);
        assert!(
            button.contains(".loading(self.scanning)"),
            "a rescan with no visible state reads as an app that ignored the click"
        );
    }

    /// …and it must stay spinning long enough to be seen.
    #[test]
    fn the_spinner_has_a_minimum_visible_duration() {
        let source = crate::views::production_source(include_str!("harness.rs"));
        let body = source
            .split_once("pub fn refresh")
            .expect("refresh must exist")
            .1;
        let body = body.split("\n    fn apply").next().unwrap_or(body);
        assert!(
            body.contains("timer(SPINNER_FLOOR)"),
            "a scan that returns in 3ms would flash the spinner for one frame, \
             which reads as a glitch rather than as feedback"
        );
        // The results must not wait on the floor — only the flag does.
        let (before_apply, after_apply) = body
            .split_once("this.apply(")
            .expect("refresh must apply the scan results");
        assert!(
            !before_apply.contains("floor.await"),
            "the floor must delay the spinner, never the results"
        );
        assert!(
            after_apply.contains("floor.await"),
            "the flag must be cleared after the floor elapses, not before"
        );
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
}
