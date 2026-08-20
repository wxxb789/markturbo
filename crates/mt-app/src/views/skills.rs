//! Skills Explorer and Inspector.
//!
//! Agent Skills are first-class here: a skill is a directory, not a filename.
//! The list shows every skill discovered across the workspace's conventional
//! roots; selecting one exposes its metadata, validation state, and files, then
//! opens `SKILL.md` in the document view.

use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    v_flex,
};
use mt_doc::{Origin, Severity, Skill, skill};

use crate::settings::{AppSettings, GroupBy};

/// Emitted when the user wants to open a skill's entry document.
#[derive(Debug, Clone)]
pub enum SkillsEvent {
    OpenFile(PathBuf),
}

pub struct SkillsView {
    focus_handle: FocusHandle,
    root: PathBuf,
    skills: Vec<Skill>,
    selected: Option<usize>,
    /// True until the first scan lands, so an empty list is not mistaken for
    /// "no skills" while the scan is still running.
    scanning: bool,
    _scan: Option<Task<()>>,
}

impl SkillsView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root,
            skills: Vec::new(),
            selected: None,
            scanning: true,
            _scan: None,
        };
        this.refresh(cx);
        this
    }

    /// Rediscover skills from disk, off the UI thread.
    ///
    /// Discovery now covers every harness's workspace directory plus the global
    /// ones, and it runs on a filesystem-watcher tick — doing that synchronously
    /// would stutter the window on every save.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let settings = AppSettings::global(cx);
        let options = mt_doc::Discovery {
            global: settings.skills_include_global,
            include_internal: settings.skills_include_internal,
        };
        self.scanning = true;
        // Replacing the task cancels any scan still in flight, so a burst of
        // filesystem events costs one scan rather than one per event.
        self._scan = Some(cx.spawn(async move |this, cx| {
            let skills = cx
                .background_spawn(async move { skill::discover_with(&root, options) })
                .await;
            crate::views::try_update(&this, cx, |this, cx| this.apply(skills, cx));
        }));
        cx.notify();
    }

    fn apply(&mut self, skills: Vec<Skill>, cx: &mut Context<Self>) {
        // Remember the selection by identity, not index: a rediscovery can
        // reorder the list.
        let previous = self
            .selected
            .and_then(|ix| self.skills.get(ix))
            .map(|s| s.dir.clone());
        self.skills = skills;
        self.scanning = false;
        self.selected = previous
            .and_then(|dir| self.skills.iter().position(|s| s.dir == dir))
            .or(if self.skills.is_empty() {
                None
            } else {
                Some(0)
            });
        cx.notify();
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// Redraw without rescanning.
    ///
    /// `selected` is an index into `self.skills`, which regrouping does not
    /// reorder — only the rendered order changes — so the selection survives on
    /// its own and this is just a notify with a name that says why.
    fn keep_selection_stable(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn selected_skill(&self) -> Option<&Skill> {
        self.selected.and_then(|ix| self.skills.get(ix))
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.skills.is_empty() {
            let hint = if self.scanning {
                "Scanning…".to_string()
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
                .p_4()
                .gap_2()
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
            .p_1()
            .gap_0p5()
            .children(rows.into_iter().map(|Row { ix, heading }| {
                let skill = &self.skills[ix];
                let selected = self.selected == Some(ix);
                let invalid = !skill.is_valid();
                v_flex()
                    .gap_0p5()
                    .children(heading.map(|heading| {
                        div()
                            .px_2()
                            .pt_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(heading)
                    }))
                    .child(
                        ListItem::new(ix)
                            .w_full()
                            .px_2()
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(ix);
                                cx.notify();
                            })),
                    )
            }))
            .into_any_element()
    }

    fn render_inspector(&self, cx: &Context<Self>) -> impl IntoElement {
        let Some(skill) = self.selected_skill() else {
            return div().into_any_element();
        };

        let entry = skill.entry.clone();
        v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().font_bold().child(skill.name.clone()))
                    .child(div().flex_1())
                    .child(
                        Button::new("open-skill")
                            .label("Open SKILL.md")
                            .xsmall()
                            .primary()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SkillsEvent::OpenFile(entry.clone()));
                            })),
                    ),
            )
            .child(div().text_xs().child(skill.summary().to_string()))
            .children(field(cx, "Origin", skill.origin.label()))
            .children(field(
                cx,
                "Location",
                &located(&self.root, skill.origin, &skill.dir),
            ))
            .children(field(
                cx,
                "Discovered in",
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
                    .child(label(cx, "Files"))
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
                    .child(label(cx, "Validation"))
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

impl EventEmitter<SkillsEvent> for SkillsView {}

impl Focusable for SkillsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SkillsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.skills.len();
        let invalid = self.skills.iter().filter(|s| !s.is_valid()).count();
        let group_by = AppSettings::global(cx).skills_group_by;

        v_flex()
            .id("skills")
            .role(gpui::Role::List)
            .aria_label("Agent skills")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("SKILLS ({count})")),
                    )
                    .when(invalid > 0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(format!("{invalid} invalid")),
                        )
                    })
                    .child(
                        Button::new("rescan")
                            .icon(IconName::Redo)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            // Grouping is a view choice, so it belongs next to the list rather
            // than buried in settings — but it persists there, because a user
            // who groups by harness means it next time too.
            .child(
                h_flex()
                    .px_3()
                    .pb_2()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Group by"),
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
                                // loaded; rescanning the filesystem for a view
                                // change would be gratuitous.
                                this.keep_selection_stable(cx);
                            }))
                    })),
            )
            .child(
                div()
                    .id("skill-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_list(cx)),
            )
            .child(self.render_inspector(cx))
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{Row, group};
    use crate::settings::GroupBy;
    use mt_doc::skill::{Skill, SkillMeta};
    use mt_doc::{Diagnostic, Origin};
    use std::path::PathBuf;

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
}
