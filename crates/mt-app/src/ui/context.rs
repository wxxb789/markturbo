//! Owner lane: context.
//!
//! Effective Agent Context in the field: the ordered sources that apply to
//! this file (global → repository root → nested), their precedence, the
//! sources a profile verified versus the inventory that never claims to
//! apply, and the `Include in review` choices.
//!
//! Contract: [`is_open`] says the field is the context's; [`field`] draws it.
//! Only outbound data emits, and only in consent — a source marked
//! `Include in review` is white-hot there, never here; everything in this
//! file is ambient ink. The order along the list is the order Codex applies
//! instructions, and the numbers are that order, not a heat ramp.
//!
//! The copy is the spec's (`agent_context_visual`): 'Context', 'Applies to
//! this file, in this order', 'Later overrides earlier', 'Include in
//! review', 'Not applied by Codex'. Resolution is local, so nothing glows
//! until a source is chosen for the next send.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use mt_core::{
    Severity,
    agent_artifacts::{
        context::{ResolutionStatus, ResolvedContext, SourceOccurrence},
        instruction::Instruction,
        skill::Origin,
    },
    settings::Language,
};
use nsh_base::{button, checkbox};
use nsh_core::{Color, prelude::*};

use crate::{
    i18n::{self, Key},
    state::AppState,
    ui::{
        motion::HOVER_MS,
        prims,
        tokens::{
            self, FONT_SOURCE, FONT_UI, HAIRLINE, SPACE_1, SPACE_2, SPACE_4, SPACE_5, TYPE_SOURCE,
            TYPE_SOURCE_LINE, TYPE_UI, TYPE_UI_LINE,
        },
    },
};

/// The order-number gutter: one column, so the numbers line up and the scope
/// lines hang under the paths.
const ORDINAL_W: f32 = 20.0;

/// Whether the field is showing the agent context.
pub fn is_open(state: &AppState) -> bool {
    state.context_open.get()
}

/// The context field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let rows_state = state.clone();
    let refresh_state = state.clone();
    let close_state = state.clone();
    view()
        .test_id("context-field")
        .w_full()
        .h(move || px(field_height()))
        .flex_col()
        .pt(px(SPACE_5))
        .child(header(state.clone(), language))
        .child(
            scroll()
                .w_full()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .gap(px(SPACE_4))
                .pt(px(SPACE_4))
                .child(busy_line(state.clone(), language))
                .child(error_line(state.clone()))
                .child(diagnostics(state.clone()))
                .child(no_sources_line(state.clone(), language))
                .child(each(
                    move || source_rows(&rows_state, language),
                    |row: &SourceRow| row.key.clone(),
                    {
                        let state = state.clone();
                        move |row, _| source_block(state.clone(), language, row)
                    },
                ))
                .child(instructions_section(state.clone(), language))
                .child(skills_section(state.clone(), language))
                .child(project_root_line(state.clone(), language)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_4))
                .pt(px(SPACE_4))
                .child(review_action(state.clone(), language))
                .child(prims::quiet_action(
                    "context-refresh",
                    i18n::t(language, Key::Refresh),
                    move || refresh_state.refresh_context(),
                ))
                .child(prims::quiet_action(
                    "context-close",
                    i18n::t(language, Key::Close),
                    move || close_state.close_context(),
                )),
        )
}

/// The field area's own height: the window under the title row.
///
/// nsh lays out with React Native's defaults, where a column's items keep
/// their content size (no stretch, no percentage height against an
/// indefinite parent), so the shell's field column — a content-sized child —
/// cannot hand its height down. A surface that needs the field's height asks
/// the window, as this one does; the scroll below then owns the rest.
fn field_height() -> f32 {
    let insets = prims::caption_insets();
    (window().height.0 - tokens::TITLE_ROW_H.max(insets.top.0)).max(0.0)
}

/// The surface's own title and the two precedence captions: the list is the
/// order, 'Later overrides earlier' says what the order means.
fn header(state: AppState, language: Language) -> impl IntoElement {
    let caption_state = state.clone();
    let later_state = state.clone();
    view()
        .flex_col()
        .flex_shrink_0()
        .gap(px(SPACE_1))
        .child(prims::ink(s(S::Context, language)))
        .child(when(
            move || source_count(&caption_state) > 0,
            move || prims::rest(s(S::OrderCaption, language)),
        ))
        .child(when(
            move || source_count(&later_state) > 1,
            move || prims::faint(s(S::LaterOverrides, language)),
        ))
}

/// How many sources the applied chain holds right now.
fn source_count(state: &AppState) -> usize {
    state
        .context_resolved
        .get()
        .map_or(0, |resolved| resolved.sources.len())
}

/// 'Resolving instructions…' while the local scan runs.
fn busy_line(state: AppState, language: Language) -> impl IntoElement {
    when(
        move || state.context_busy.get(),
        move || prims::rest(i18n::t(language, Key::ResolvingInstructions)),
    )
}

/// The resolution failure, when there is one.
fn error_line(state: AppState) -> impl IntoElement {
    let message_state = state.clone();
    when(
        move || state.context_error.get().is_some(),
        move || danger_text(move || message_state.context_error.get().unwrap_or_default()),
    )
}

/// Resolution diagnostics: what could not be read or is not followed. Their
/// messages are the resolver's, shown as they are; errors in the danger ink,
/// warnings at resting ink. Informational notices are not shown here — the
/// rows carry size and truncation already.
///
/// Every conditional block of this surface is a `when` region and not an
/// empty view: a region's content goes, an empty view would still take a gap.
fn diagnostics(state: AppState) -> impl IntoElement {
    let condition_state = state.clone();
    let rows_state = state.clone();
    when(
        move || !diagnostic_rows(&condition_state).is_empty(),
        move || {
            let rows_state = rows_state.clone();
            view().flex_col().gap(px(SPACE_2)).child(each(
                move || diagnostic_rows(&rows_state),
                |row: &DiagnosticRow| row.key.clone(),
                move |row, _| {
                    let danger = row.get().danger;
                    text(move || row.get().message.clone())
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || {
                            if danger {
                                tokens::palette().danger
                            } else {
                                tokens::palette().ink_rest
                            }
                        })
                },
            ))
        },
    )
}

/// 'No AGENTS.md applies' once a resolution settled on an empty chain: the
/// calm room must not read as a failure.
fn no_sources_line(state: AppState, language: Language) -> impl IntoElement {
    when(
        move || {
            !state.context_busy.get()
                && state.context_error.get().is_none()
                && state
                    .context_resolved
                    .get()
                    .is_some_and(|resolved| resolved.sources.is_empty())
        },
        move || prims::faint(s(S::NoSources, language)),
    )
}

/// The applied chain, in the order Codex applies it.
fn source_rows(state: &AppState, language: Language) -> Vec<SourceRow> {
    let Some(resolved) = state.context_resolved.get() else {
        return Vec::new();
    };
    resolved
        .sources
        .iter()
        .enumerate()
        .map(|(index, source)| SourceRow::new(&resolved, index, source, language))
        .collect()
}

/// One applied source, as its row draws it.
#[derive(Clone, PartialEq)]
struct SourceRow {
    /// The row's identity: its chain position and lexical path.
    key: SharedString,
    index: usize,
    number: usize,
    path: PathBuf,
    path_label: SharedString,
    detail: SharedString,
    includable: bool,
}

impl SourceRow {
    fn new(
        resolved: &ResolvedContext,
        index: usize,
        source: &SourceOccurrence,
        language: Language,
    ) -> Self {
        let includable = source
            .content_index
            .and_then(|content| resolved.contents.get(content))
            .is_some_and(|content| !content.text.trim().is_empty());
        Self {
            key: SharedString::from(format!("{index}:{}", source.path.display())),
            index,
            number: index + 1,
            path: source.path.clone(),
            path_label: SharedString::from(display_path(resolved, source)),
            detail: SharedString::from(format!(
                "{} \u{b7} {}",
                scope_line(resolved, source, language),
                bytes_line(resolved, source, includable, language),
            )),
            includable,
        }
    }
}

/// One source: its order number, its provenance path (a press opens the
/// file), the scope and bytes it contributes, and the one toggle.
fn source_block(
    state: AppState,
    language: Language,
    row: ReadSignal<SourceRow>,
) -> impl IntoElement {
    let detail_row = row;
    view()
        .flex_col()
        .gap(px(SPACE_1))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_2))
                .child(ordinal(row))
                .child(path_press(state.clone(), language, row))
                .child(include_toggle(state, language, row)),
        )
        .child(
            view()
                .pl(px(ORDINAL_W + SPACE_2))
                .child(prims::faint(move || detail_row.get().detail.clone())),
        )
}

/// The order number: full ink, tabular figures, in the same mono face as the
/// path so the chain reads as one column.
fn ordinal(row: ReadSignal<SourceRow>) -> impl IntoElement {
    view().w(px(ORDINAL_W)).flex_shrink_0().child(
        text(move || row.get().number.to_string())
            .font_family(FONT_SOURCE)
            .text_size(px(TYPE_SOURCE))
            .line_height(px(TYPE_SOURCE_LINE))
            .tabular_nums()
            .text_color(move || tokens::palette().ink),
    )
}

/// The provenance path: resting ink, a press opens the file as a document.
fn path_press(state: AppState, language: Language, row: ReadSignal<SourceRow>) -> impl IntoElement {
    let index = row.get_untracked().index;
    let path = row.get_untracked().path.clone();
    view()
        .test_id(&format!("context-source-{index}"))
        .flex_1()
        .min_w(px(0.0))
        .bg(Color::TRANSPARENT)
        .child(
            button()
                .accessibility_label(move || {
                    Some(SharedString::from(open_label(
                        language,
                        &row.get_untracked().path_label,
                    )))
                })
                .w_full()
                .px(px(0.0))
                .py(px(0.0))
                .bg(Color::TRANSPARENT)
                .rounded(px(0.0))
                .cursor_pointer()
                .transition_colors()
                .duration(HOVER_MS)
                .hover(|style| style.text_color(tokens::palette().ink).underline())
                .focus_visible(|style| style.text_color(tokens::palette().ink).underline())
                .on_press(move || state.open_file(path.clone()))
                .child(
                    text(move || row.get().path_label.clone())
                        .font_family(FONT_SOURCE)
                        .text_size(px(TYPE_SOURCE))
                        .line_height(px(TYPE_SOURCE_LINE))
                        .text_color(move || tokens::palette().ink_rest)
                        .w_full()
                        .truncate(),
                ),
        )
}

/// The `Include in review` toggle: ambient ink only — a checked square and
/// full-ink label. What it marks is left to leave the device in consent;
/// here nothing glows. Only a source with effective text can be included,
/// so a content-free occurrence has no toggle.
fn include_toggle(
    state: AppState,
    language: Language,
    row: ReadSignal<SourceRow>,
) -> impl IntoElement {
    let index = row.get_untracked().index;
    let selected_signal = state.context_selected;
    let changed_state = state.clone();
    // Only `Copy` captures, so the closure itself is `Copy` and every binding
    // below can own one.
    let selected = move || selected_signal.get().contains(&index);
    let name = move || {
        Some(SharedString::from(i18n::effective_context_choice_label(
            language,
            selected_signal.get().contains(&index),
        )))
    };
    when(
        move || row.get().includable,
        move || {
            let changed = changed_state.clone();
            // The toggle is `nsh_base`'s checkbox — it owns the presses,
            // the keyboard activation and the checked state for screen
            // readers; the id lives on the view around it, and a press
            // names that view's text (see the module's test).
            view()
                .test_id(&format!("context-include-{index}"))
                .flex_shrink_0()
                .bg(Color::TRANSPARENT)
                .child(
                    checkbox()
                        .checked(selected)
                        .on_change(move |next: &bool| {
                            if *next != changed.context_selected.get().contains(&index) {
                                changed.toggle_context_source(index);
                            }
                        })
                        .accessibility_label(name)
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_2))
                        .px(px(SPACE_1))
                        .py(px(SPACE_1))
                        .bg(Color::TRANSPARENT)
                        .cursor_pointer()
                        .transition_colors()
                        .duration(HOVER_MS)
                        .hover(|style| style.underline())
                        .focus_visible(|style| style.underline())
                        .indicator(
                            view()
                                .w(px(11.0))
                                .h(px(11.0))
                                .border(px(HAIRLINE))
                                .border_color(move || {
                                    if selected() {
                                        tokens::palette().ink
                                    } else {
                                        tokens::palette().ink_faint
                                    }
                                })
                                .bg(move || {
                                    if selected() {
                                        tokens::palette().ink
                                    } else {
                                        Color::TRANSPARENT
                                    }
                                }),
                        )
                        .child(
                            text(s(S::IncludeInReview, language))
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || {
                                    if selected() {
                                        tokens::palette().ink
                                    } else {
                                        tokens::palette().ink_rest
                                    }
                                }),
                        ),
                )
        },
    )
}

/// 'Not applied by Codex': the instruction files discovery found that this
/// profile does not apply. They carry no number and no toggle, never emit,
/// and never claim to apply; a press opens the file.
fn instructions_section(state: AppState, language: Language) -> impl IntoElement {
    let condition_state = state.clone();
    let label_state = state.clone();
    let rows_state = state.clone();
    when(
        move || !instruction_rows(&condition_state).is_empty(),
        move || {
            let label_state = label_state.clone();
            let rows_state = rows_state.clone();
            let section_state = state.clone();
            view()
                .flex_col()
                .gap(px(SPACE_2))
                .child(section_label(
                    s(S::NotAppliedByCodex, language),
                    move || instruction_rows(&label_state).len().to_string(),
                ))
                .child(each(
                    move || instruction_rows(&rows_state),
                    |row: &InventoryRow| row.key.clone(),
                    move |row, _| inventory_row(section_state.clone(), row, instruction_ink),
                ))
        },
    )
}

/// The skills discovery found: an inventory read on demand, so it never adds
/// bodies to any send.
fn skills_section(state: AppState, language: Language) -> impl IntoElement {
    let condition_state = state.clone();
    let label_state = state.clone();
    let rows_state = state.clone();
    when(
        move || !skill_rows(&condition_state).is_empty(),
        move || {
            let label_state = label_state.clone();
            let rows_state = rows_state.clone();
            let section_state = state.clone();
            view()
                .flex_col()
                .gap(px(SPACE_2))
                .child(section_label(
                    i18n::t(language, Key::SectionSkills),
                    move || skill_rows(&label_state).len().to_string(),
                ))
                .child(each(
                    move || skill_rows(&rows_state),
                    |row: &InventoryRow| row.key.clone(),
                    move |row, _| inventory_row(section_state.clone(), row, skill_ink),
                ))
        },
    )
}

/// Afterimage ink: what discovery found but this profile does not apply.
fn instruction_ink() -> Color {
    tokens::palette().afterimage_ink
}

/// Resting ink: the skills inventory.
fn skill_ink() -> Color {
    tokens::palette().ink_rest
}

/// One line of an inventory list: a name, its origin, and the file a press
/// opens.
#[derive(Clone, PartialEq)]
struct InventoryRow {
    /// The row's test id: one per list position, so a selector names a row.
    id: SharedString,
    key: SharedString,
    label: SharedString,
    origin: SharedString,
    path: PathBuf,
}

fn instruction_rows(state: &AppState) -> Vec<InventoryRow> {
    let applied = state
        .context_resolved
        .get()
        .map(|resolved| applied_paths(&resolved))
        .unwrap_or_default();
    state
        .harness_instructions
        .get()
        .into_iter()
        .filter(|instruction| !is_applied(&applied, instruction))
        .enumerate()
        .map(|(index, instruction)| InventoryRow {
            id: SharedString::from(format!("context-instruction-{index}")),
            key: SharedString::from(format!("instruction:{}", instruction.path.display())),
            label: SharedString::from(instruction.label()),
            origin: SharedString::from(instruction.origin.label()),
            path: instruction.path,
        })
        .collect()
}

fn skill_rows(state: &AppState) -> Vec<InventoryRow> {
    state
        .harness_skills
        .get()
        .into_iter()
        .enumerate()
        .map(|(index, skill)| InventoryRow {
            id: SharedString::from(format!("context-skill-{index}")),
            key: SharedString::from(format!("skill:{}", skill.entry.display())),
            label: SharedString::from(skill.name),
            origin: SharedString::from(skill.origin.label()),
            path: skill.entry,
        })
        .collect()
}

/// The paths the profile applies, as discovery found them: the lexical path
/// and, when the resolver recorded one, the physical path behind a link.
fn applied_paths(resolved: &ResolvedContext) -> HashSet<PathBuf> {
    let mut applied = HashSet::new();
    for source in &resolved.sources {
        applied.insert(source.path.clone());
        if let Some(physical) = &source.physical_path {
            applied.insert(physical.clone());
        }
    }
    applied
}

/// Whether discovery found this instruction file in the applied chain — by
/// the file's own path or one of the aliases a harness directory resolved to.
fn is_applied(applied: &HashSet<PathBuf>, instruction: &Instruction) -> bool {
    applied.contains(&instruction.path)
        || instruction
            .aliases
            .iter()
            .any(|alias| applied.contains(alias))
}

/// One inventory line. `ink` is read inside the binding, so a theme change
/// repaints the row.
fn inventory_row(
    state: AppState,
    row: ReadSignal<InventoryRow>,
    ink: fn() -> Color,
) -> impl IntoElement {
    let path = row.get_untracked().path.clone();
    let id = row.get_untracked().id.clone();
    let label = move || row.get().label.clone();
    let origin = move || row.get().origin.clone();
    let name = move || {
        SharedString::from(format!(
            "{} \u{b7} {}",
            row.get_untracked().label,
            row.get_untracked().origin
        ))
    };
    view()
        .test_id(&id)
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .child(
            button()
                .accessibility_label(name)
                .px(px(0.0))
                .py(px(SPACE_1))
                .bg(Color::TRANSPARENT)
                .rounded(px(0.0))
                .cursor_pointer()
                .transition_colors()
                .duration(HOVER_MS)
                .hover(|style| style.text_color(tokens::palette().ink).underline())
                .focus_visible(|style| style.text_color(tokens::palette().ink).underline())
                .on_press(move || state.open_file(path.clone()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_2))
                        .child(
                            text(label)
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(ink),
                        )
                        .child(
                            text(origin)
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || tokens::palette().ink_faint),
                        ),
                ),
        )
}

/// A section's name and how many rows it holds.
fn section_label(label: &'static str, count: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .child(
            text(label)
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
        .child(
            text(move || format!("({})", count()))
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_faint),
        )
}

/// Where the chain was resolved: the anchor every scope line is relative to.
fn project_root_line(state: AppState, language: Language) -> impl IntoElement {
    let label_state = state.clone();
    when(
        move || {
            label_state
                .context_resolved
                .get()
                .and_then(|resolved| resolved.project_root)
                .is_some()
        },
        move || {
            let state = state.clone();
            prims::faint(move || {
                let root = state
                    .context_resolved
                    .get()
                    .and_then(|resolved| resolved.project_root)
                    .map(|root| root.display().to_string().replace('\\', "/"))
                    .unwrap_or_default();
                format!("{} \u{b7} {root}", s(S::ProjectRoot, language))
            })
        },
    )
}

/// The state's one next action: the review, with what is selected riding
/// along; with nothing selected it is the ordinary review.
fn review_action(state: AppState, language: Language) -> impl IntoElement {
    let count_state = state.clone();
    let selected_state = state.clone();
    view().flex_shrink_0().child(
        when(
            move || !count_state.context_selected.get().is_empty(),
            move || {
                let press_state = state.clone();
                prims::primary_action(
                    "context-review",
                    i18n::t(language, Key::ReviewWithContext),
                    move || press_state.review_with_context(),
                )
            },
        )
        .otherwise({
            let press_state = selected_state;
            move || {
                let press_state = press_state.clone();
                prims::primary_action(
                    "context-review",
                    i18n::t(language, Key::Review),
                    move || press_state.review_with_context(),
                )
            }
        }),
    )
}

/// The scope a source in the chain covers: a global source applies to every
/// project, a project source from its directory through the execution cwd.
fn scope_line(resolved: &ResolvedContext, source: &SourceOccurrence, language: Language) -> String {
    if source.origin == Origin::Global {
        return s(S::AppliesEverywhere, language).to_owned();
    }
    let root = project_root(resolved);
    match relative_display(&source.scope, &root) {
        Some(relative) if relative.is_empty() => s(S::AppliesFromRoot, language).to_owned(),
        Some(relative) => scope_under(language, &relative),
        None => scope_under(language, &display(&source.scope)),
    }
}

/// What the occurrence's bytes contribute to the chain, and why nothing can
/// when there is nothing to include.
fn bytes_line(
    resolved: &ResolvedContext,
    source: &SourceOccurrence,
    includable: bool,
    language: Language,
) -> String {
    if !includable {
        return match (resolved.status, source.origin) {
            (ResolutionStatus::Partial, Origin::Workspace) => {
                s(S::ContentUnavailable, language).to_owned()
            }
            _ => s(S::Empty, language).to_owned(),
        };
    }
    if source.included_bytes == source.raw_bytes {
        format!("{} B", grouped(source.raw_bytes))
    } else {
        match language {
            Language::English => format!(
                "{} of {} B",
                grouped(source.included_bytes),
                grouped(source.raw_bytes)
            ),
            Language::Chinese => format!(
                "{} / {} B",
                grouped(source.included_bytes),
                grouped(source.raw_bytes)
            ),
        }
    }
}

/// The provenance path as the row shows it: `~` for a home-anchored global
/// source, the path relative to the project root for a project one, the
/// absolute path otherwise. `/` everywhere.
fn display_path(resolved: &ResolvedContext, source: &SourceOccurrence) -> String {
    let base = match source.origin {
        Origin::Global => dirs::home_dir(),
        Origin::Workspace => Some(project_root(resolved)),
    };
    match base
        .as_deref()
        .and_then(|base| relative_display(&source.path, base))
    {
        Some(relative) if !relative.is_empty() => match source.origin {
            Origin::Global => format!("~/{relative}"),
            Origin::Workspace => relative,
        },
        _ => display(&source.path),
    }
}

/// `path` as `/`-separated text.
fn display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// `path` relative to `base`, `/`-separated; `None` when it is not inside
/// (then both are compared as written, the resolver's lexical identity).
fn relative_display(path: &Path, base: &Path) -> Option<String> {
    path.strip_prefix(base).ok().map(display)
}

/// The project root, or the workspace when no marker established one.
fn project_root(resolved: &ResolvedContext) -> PathBuf {
    resolved
        .project_root
        .clone()
        .unwrap_or_else(|| resolved.input.workspace.clone())
}

/// 'Applies under <dir>/': the directory an instruction covers on its way to
/// the execution cwd.
fn scope_under(language: Language, directory: &str) -> String {
    let directory = directory.trim_end_matches('/');
    match language {
        Language::English => format!("Applies under {directory}/"),
        Language::Chinese => format!("适用于 {directory}/ 下的内容"),
    }
}

/// Group a byte count in threes, as the consent manifest does.
fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The accessible name of a source's path: what pressing it does.
fn open_label(language: Language, path: &str) -> String {
    match language {
        Language::English => format!("Open {path}"),
        Language::Chinese => format!("打开 {path}"),
    }
}

/// A row of the diagnostics list.
#[derive(Clone, PartialEq)]
struct DiagnosticRow {
    key: SharedString,
    message: SharedString,
    danger: bool,
}

fn diagnostic_rows(state: &AppState) -> Vec<DiagnosticRow> {
    let Some(resolved) = state.context_resolved.get() else {
        return Vec::new();
    };
    resolved
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity != Severity::Info)
        .map(|diagnostic| DiagnosticRow {
            key: SharedString::from(diagnostic.to_string()),
            message: SharedString::from(diagnostic.message.clone()),
            danger: diagnostic.severity == Severity::Error,
        })
        .collect()
}

/// A danger-ink line; the foundation's primitives stop at faint.
fn danger_text(content: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().danger)
}

/// The surface's copy. New strings live here rather than in `i18n.rs` (a
/// merge hotspot): the match is exhaustive, so a missing translation is a
/// compile error.
#[derive(Clone, Copy)]
enum S {
    Context,
    OrderCaption,
    LaterOverrides,
    IncludeInReview,
    NotAppliedByCodex,
    NoSources,
    AppliesEverywhere,
    AppliesFromRoot,
    Empty,
    ContentUnavailable,
    ProjectRoot,
}

fn s(key: S, language: Language) -> &'static str {
    match (key, language) {
        (S::Context, Language::English) => "Context",
        (S::Context, Language::Chinese) => "上下文",
        (S::OrderCaption, Language::English) => "Applies to this file, in this order",
        (S::OrderCaption, Language::Chinese) => "按此顺序应用于此文件",
        (S::LaterOverrides, Language::English) => "Later overrides earlier",
        (S::LaterOverrides, Language::Chinese) => "后者覆盖前者",
        (S::IncludeInReview, Language::English) => "Include in review",
        (S::IncludeInReview, Language::Chinese) => "包含在审查中",
        (S::NotAppliedByCodex, Language::English) => "Not applied by Codex",
        (S::NotAppliedByCodex, Language::Chinese) => "Codex 未应用",
        (S::NoSources, Language::English) => "No AGENTS.md file applies to this file",
        (S::NoSources, Language::Chinese) => "没有 AGENTS.md 文件适用于此文件",
        (S::AppliesEverywhere, Language::English) => "Applies to every project",
        (S::AppliesEverywhere, Language::Chinese) => "适用于所有项目",
        (S::AppliesFromRoot, Language::English) => "Applies from the repository root",
        (S::AppliesFromRoot, Language::Chinese) => "从仓库根目录起适用",
        (S::Empty, Language::English) => "empty",
        (S::Empty, Language::Chinese) => "无内容",
        (S::ContentUnavailable, Language::English) => "content unavailable",
        (S::ContentUnavailable, Language::Chinese) => "内容不可用",
        (S::ProjectRoot, Language::English) => "Project root",
        (S::ProjectRoot, Language::Chinese) => "项目根目录",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nsh_engine::{EngineConfig, Points, Root};
    use nsh_testing::Headless;

    use super::*;

    /// The two tests share one process: each holds this while its fixture
    /// and its environment are the ones its probe reads.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A fixture with one source of every kind — global, repository root and
    /// nested in the applied chain; CLAUDE.md, a Cursor rule file and a
    /// skill in the inventory below it — and the probe environment for it.
    struct Fixture {
        root: PathBuf,
        _root: tempfile::TempDir,
        _codex_home: tempfile::TempDir,
        _config: tempfile::TempDir,
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl Fixture {
        fn new() -> Self {
            let serial = SERIAL
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let root = tempfile::tempdir().expect("fixture");
            let root_path = root.path().to_path_buf();
            std::fs::create_dir_all(root_path.join("docs")).expect("docs");
            std::fs::create_dir_all(root_path.join(".git")).expect("root marker");
            std::fs::create_dir_all(root_path.join(".cursor/rules")).expect("cursor rules");
            std::fs::create_dir_all(root_path.join(".claude/skills/fix-bug")).expect("skill dir");
            std::fs::write(root_path.join("AGENTS.md"), "Project rules.\n")
                .expect("project instructions");
            std::fs::write(root_path.join("docs/AGENTS.md"), "Docs rules.\n")
                .expect("nested instructions");
            std::fs::write(root_path.join("docs/plan.md"), "# Plan\n").expect("document");
            std::fs::write(root_path.join("CLAUDE.md"), "Claude rules.\n")
                .expect("claude instructions");
            std::fs::write(root_path.join(".cursor/rules/style.mdc"), "Be terse.\n")
                .expect("cursor rule");
            std::fs::write(
                root_path.join(".claude/skills/fix-bug/SKILL.md"),
                "---\nname: fix-bug\ndescription: Fix one bug.\n---\n\nDo it.\n",
            )
            .expect("skill");
            let codex_home = tempfile::tempdir().expect("codex home");
            std::fs::write(codex_home.path().join("AGENTS.md"), "Global rules.\n")
                .expect("global instructions");
            let config = tempfile::tempdir().expect("config");
            // SAFETY: both tests hold `SERIAL` from here on, and this module
            // is the only place in the binary that touches these variables.
            unsafe {
                std::env::set_var("MT_CONTEXT_FIXTURE", &root_path);
                std::env::set_var("CODEX_HOME", codex_home.path());
                std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
                std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
            }
            Self {
                root: root_path,
                _root: root,
                _codex_home: codex_home,
                _config: config,
                _serial: serial,
            }
        }
    }

    /// The whole shell over the fixture, with the context field open.
    #[component]
    fn ContextProbe() -> impl IntoElement {
        let state = AppState::new();
        let root = PathBuf::from(std::env::var_os("MT_CONTEXT_FIXTURE").expect("fixture root"));
        state.open_folder(root.clone());
        state.open_file(root.join("docs").join("plan.md"));
        state.toggle_context();
        view().flex_col().child(crate::ui::shell::shell(
            state,
            Language::English,
            crate::ui::tokens::Layout::Wide,
        ))
    }

    /// The field alone, without the shell around it: the shell's overlay
    /// layer covers the whole window in the harness's hit testing, so a press
    /// reaches the surface's controls only where nothing is over them.
    #[component]
    fn FieldProbe() -> impl IntoElement {
        let state = AppState::new();
        let root = PathBuf::from(std::env::var_os("MT_CONTEXT_FIXTURE").expect("fixture root"));
        state.open_folder(root.clone());
        state.open_file(root.join("docs").join("plan.md"));
        state.toggle_context();
        let field_state = state.clone();
        let open_state = state.context_open;
        let selected = state.context_selected;
        let tabs = state.tabs;
        view()
            .flex_col()
            .child(when(
                move || open_state.get(),
                move || crate::ui::context::field(field_state.clone(), Language::English),
            ))
            .child(text(move || format!("selected:{}", selected.get().len())))
            .child(text(move || format!("tabs:{}", tabs.get().len())))
    }

    /// The shell routes the field to the context's surface, which lists the
    /// applied chain in the order Codex applies it — global, repository root,
    /// then the nested directory — and the inventory below it, over the whole
    /// height of the field.
    #[test]
    fn renders_the_applied_chain_and_the_inventory() {
        let _fixture = Fixture::new();
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ContextProbe>())).expect("start the app");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("wide viewport");
        app.wait_for("id:context-field").expect("the context field");
        app.wait_for("id:context-include-2")
            .expect("the three applied sources");

        let shown = app.screen().texts().join("\n");
        let at = |needle: &str| {
            shown
                .find(needle)
                .unwrap_or_else(|| panic!("`{needle}` missing:\n{shown}"))
        };
        assert!(
            shown.contains("Applies to this file, in this order"),
            "{shown}"
        );
        assert!(shown.contains("Later overrides earlier"), "{shown}");
        assert!(shown.contains("Applies to every project"), "{shown}");
        assert!(
            shown.contains("Applies from the repository root"),
            "{shown}"
        );
        assert!(shown.contains("Applies under docs/"), "{shown}");
        assert!(shown.contains("Include in review"), "{shown}");
        assert!(shown.contains("Not applied by Codex"), "{shown}");
        assert!(shown.contains("CLAUDE.md"), "{shown}");
        assert!(shown.contains(".cursor/rules/style.mdc"), "{shown}");
        assert!(shown.contains("fix-bug"), "{shown}");
        // The order along the list is the order Codex applies it, and the
        // inventory sits below the applied chain, skills below it.
        assert!(at("Applies to every project") < at("Applies from the repository root"));
        assert!(at("Applies from the repository root") < at("Applies under docs/"));
        assert!(at("Applies under docs/") < at("Not applied by Codex"));
        assert!(at("Not applied by Codex") < at("fix-bug"));
        assert_eq!(app.screen().count("id:context-include-2").unwrap(), 1);

        // The surface fills the field under the title row, the rows sit in
        // it and the actions below them: a percentage height would collapse
        // under nsh's React Native defaults and hide every row.
        let nodes = app.screen().layout_nodes();
        let bounds = |id: &str| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(id))
                .map(|node| node.bounds)
                .unwrap_or_else(|| panic!("`{id}` on screen"))
        };
        let field_box = bounds("context-field");
        assert!(field_box.size.height.0 > 600.0, "{field_box:?}");
        assert!(field_box.size.width.0 > 300.0, "{field_box:?}");
        let first_row = bounds("context-include-0");
        let actions = bounds("context-close");
        // The state's one next action is on screen too.
        let _review = bounds("context-review");
        assert!(
            field_box.origin.y.0 + field_box.size.height.0 <= 900.0,
            "{field_box:?}"
        );
        assert!(first_row.origin.y.0 > field_box.origin.y.0, "{first_row:?}");
        assert!(
            first_row.origin.y.0 + first_row.size.height.0 < actions.origin.y.0,
            "the rows above the actions: {first_row:?} {actions:?}"
        );
        assert!(
            actions.origin.y.0 > 700.0,
            "the actions at the field's foot: {actions:?}"
        );
    }

    /// The surface's controls: a press on a path opens the file, a press on
    /// the toggle records the source for the next send, Refresh re-resolves
    /// the chain, and Close leaves the field.
    #[test]
    fn toggles_refreshes_opens_and_closes() {
        let fixture = Fixture::new();
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<FieldProbe>())).expect("start the app");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("wide viewport");
        app.wait_for("id:context-include-2")
            .expect("the three applied sources");

        // Include, then exclude, the global source. The id sits on the view
        // around the checkbox, so the press names the checkbox's own label.
        app.press("text:Include in review >> nth=0")
            .expect("include the global source");
        app.wait_for("text:selected:1")
            .expect("the selection recorded");
        app.press("text:Include in review >> nth=0")
            .expect("include it again");
        app.wait_for("text:selected:0")
            .expect("the selection cleared");

        // Refresh resolves again: an override file that did not exist before
        // joins the chain in place of the docs instructions.
        std::fs::write(fixture.root.join("docs/AGENTS.override.md"), "Override.\n")
            .expect("override instructions");
        app.press("label:Refresh").expect("refresh the chain");
        app.wait_for("text:docs/AGENTS.override.md")
            .expect("the refreshed chain");

        // A path press opens the source as a document.
        app.press("label:Open AGENTS.md")
            .expect("open the repository's AGENTS.md");
        app.wait_for("text:tabs:2").expect("the source opened");

        // Close leaves the field.
        app.press("label:Close").expect("close the context");
        app.wait_for_gone("id:context-field", Duration::from_secs(5))
            .expect("the field closed");
    }
}
