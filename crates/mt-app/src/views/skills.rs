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
use mt_doc::{Severity, Skill, skill};

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
}

impl SkillsView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root,
            skills: Vec::new(),
            selected: None,
        };
        this.refresh(cx);
        this
    }

    /// Rediscover skills from disk.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // Remember the selection by identity, not index: a rediscovery can
        // reorder the list.
        let previous = self.selected.and_then(|ix| self.skills.get(ix)).map(|s| s.dir.clone());
        self.skills = skill::discover(&self.root);
        self.selected = previous
            .and_then(|dir| self.skills.iter().position(|s| s.dir == dir))
            .or(if self.skills.is_empty() { None } else { Some(0) });
        cx.notify();
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    fn selected_skill(&self) -> Option<&Skill> {
        self.selected.and_then(|ix| self.skills.get(ix))
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.skills.is_empty() {
            return v_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No skills found."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Looked in: {}",
                            skill::DISCOVERY_ROOTS.join(", ")
                        )),
                )
                .into_any_element();
        }

        v_flex()
            .p_1()
            .gap_0p5()
            .children(self.skills.iter().enumerate().map(|(ix, skill)| {
                let selected = self.selected == Some(ix);
                let invalid = !skill.is_valid();
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
                    }))
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
            .child(
                div()
                    .text_xs()
                    .child(skill.summary().to_string()),
            )
            .children(field(cx, "Location", &relative(&self.root, &skill.dir)))
            .children(field(
                cx,
                "Discovered in",
                &relative(&self.root, &skill.root),
            ))
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
            .children(skill.meta.metadata.iter().filter_map(|(k, v)| {
                field(cx, k, v)
            }))
            .children(skill.meta.extra.iter().filter_map(|(k, v)| {
                field(cx, &format!("{k} (non-standard)"), v)
            }))
            // Supporting directories: `scripts/`, `references/`, `assets/`.
            .children((!skill.support_dirs.is_empty()).then(|| {
                v_flex().gap_0p5().mt_1().child(label(cx, "Files")).children(
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
                                dir.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or_default()
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
                        div()
                            .text_xs()
                            .text_color(color)
                            .child(d.message.clone())
                    }))
            }))
            .into_any_element()
    }
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

fn relative(root: &Path, path: &Path) -> String {
    crate::workspace::display_relative(root, path)
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

        v_flex()
            .id("skills")
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
