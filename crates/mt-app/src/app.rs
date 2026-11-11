//! Application root and the window's three regions: workspace, document and
//! status. Components run once; every changing value is a signal.

use std::{cell::Cell, path::PathBuf, rc::Rc, sync::LazyLock, time::Duration};

use mt_core::{
    BlockKind, Document,
    agent_artifacts::{context::SourceOccurrence, instruction::Instruction, skill::Skill},
    rendering::{RenderOutcome, RendererRegistry},
    settings::{RecentTarget, RecentTargetKind},
};
use nsh_base::{TextViewMath, button, editor, input, text_view, theme};
use nsh_core::{element::ImageSrc, file_dialog::OpenDialog, prelude::*};

use crate::state::{AppState, Pane, Tab, language_for_path};

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    let state = AppState::new();
    if let Some(path) = std::env::var_os("MARKTURBO_OPEN").map(PathBuf::from) {
        if path.is_dir() {
            state.open_folder(path);
        } else {
            if let Some(parent) = path.parent() {
                state.open_folder(parent.to_path_buf());
            }
            state.open_file(path);
        }
    }
    let tick_state = state.clone();
    let ticker_alive = Rc::new(Cell::new(true));
    let ticker_flag = ticker_alive.clone();
    let _ticker = stored(set_interval(Duration::from_secs(1), move || {
        if ticker_flag.get() {
            tick_state.checkpoint_tick();
        }
    }));
    let _ticker_guard = stored(TickerGuard(ticker_alive));
    view()
        .flex_1()
        .flex_row()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(sidebar(state.clone()))
        .child(main(state))
}

/// Marks the checkpoint ticker inactive when the window's owner is disposed,
/// so a racing interval cannot read signals that are already gone.
struct TickerGuard(Rc<Cell<bool>>);

impl Drop for TickerGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn sidebar(state: AppState) -> impl IntoElement {
    view()
        .w(px(260.0))
        .h(relative(1.0))
        .flex_col()
        .bg(move || theme().card)
        .border_r_1()
        .border_color(move || theme().border)
        .child(
            view()
                .h(px(44.0))
                .px_3()
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    text("Workspace")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    button()
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(move || theme().secondary)
                        .on_press({
                            let state = state.clone();
                            move || choose_folder(state.clone())
                        })
                        .child(text("Open").text_size(px(12.0))),
                ),
        )
        .child(search_row(state.clone()))
        .child(
            view()
                .px_2()
                .pb_2()
                .flex_row()
                .gap_1()
                .child(small_button("Review", {
                    let state = state.clone();
                    move || state.start_review()
                }))
                .child(small_button("Context", {
                    let state = state.clone();
                    move || state.toggle_context()
                }))
                .child(small_button("Settings", {
                    let state = state.clone();
                    move || state.open_settings()
                })),
        )
        .child(
            scroll().flex_1().child(
                view()
                    .flex_col()
                    .child(when(move || state.search_ran.get(), {
                        let state = state.clone();
                        move || search_results_view(state.clone()).into_element()
                    }))
                    .child(when(move || !state.search_ran.get(), {
                        let state = state.clone();
                        move || tree_view(state.clone()).into_element()
                    })),
            ),
        )
}

fn search_row(state: AppState) -> impl IntoElement {
    let change_state = state.clone();
    let submit_state = state.clone();
    let go_state = state.clone();
    view()
        .p_2()
        .flex_col()
        .gap_1()
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            input()
                .test_id("search-input")
                .value(move || state.search_query.get())
                .placeholder("Search files")
                .on_change(move |value: &str| change_state.search_query.set(value.to_owned()))
                .on_submit(move |value: &str| submit_state.run_search(value))
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .border_1()
                .border_color(move || theme().border),
        )
        .child(
            view()
                .flex_row()
                .gap_1()
                .child(small_button("Search", move || {
                    go_state.run_search(&go_state.search_query.get())
                }))
                .child(small_button("Clear", {
                    let state = state.clone();
                    move || state.clear_search()
                })),
        )
}

fn tree_view(state: AppState) -> impl IntoElement {
    each(
        move || state.entries.get(),
        |entry: &crate::state::Entry| entry.path.clone(),
        move |entry: ReadSignal<crate::state::Entry>, _| {
            let entry = entry.get();
            let state = state.clone();
            let path = entry.path.clone();
            view()
                .pl(px(8.0 + entry.depth as f32 * 14.0))
                .pr_2()
                .py(px(4.0))
                .rounded(px(4.0))
                .on_press(move || {
                    if !entry.is_dir {
                        state.open_file(path.clone());
                    }
                })
                .child(text(entry.name).text_size(px(12.0)).text_color(move || {
                    if entry.is_dir {
                        theme().muted_foreground
                    } else {
                        theme().foreground
                    }
                }))
        },
    )
}

fn search_results_view(state: AppState) -> impl IntoElement {
    let results = state.search_results;
    view()
        .flex_col()
        .child(when(
            move || state.search_busy.get(),
            || {
                text("Searching…")
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(when(
            move || state.search_truncated.get(),
            || {
                text("Showing the first results only.")
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(when(
            move || !state.search_busy.get() && results.get().is_empty(),
            || {
                text("No matches.")
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(each(
            move || results.get(),
            |hit: &crate::state::SearchHit| hit.label.clone(),
            move |hit: ReadSignal<crate::state::SearchHit>, _| {
                let hit = hit.get();
                let state = state.clone();
                view()
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .on_press(move || state.open_file(hit.path.clone()))
                    .child(
                        text(hit.label)
                            .text_size(px(11.0))
                            .text_color(move || theme().muted_foreground),
                    )
            },
        ))
}

fn main(state: AppState) -> impl IntoElement {
    let state_for_settings = state.clone();
    view()
        .flex_1()
        .h(relative(1.0))
        .flex_col()
        .child(tab_bar(state.clone()))
        .child(toolbar(state.clone()))
        .child(when(
            {
                let state = state.clone();
                move || !state.recovery_records.get().is_empty()
            },
            {
                let state = state.clone();
                move || recovery_banner(state.clone()).into_element()
            },
        ))
        .child(
            when(move || state_for_settings.settings_open.get(), {
                let state = state.clone();
                move || settings_page(state.clone()).into_element()
            })
            .otherwise({
                let state = state.clone();
                move || document_area(state.clone()).into_element()
            }),
        )
        .child(status_bar(state))
}

fn document_area(state: AppState) -> impl IntoElement {
    let state_for_when = state.clone();
    let state_for_review = state.clone();
    view()
        .flex_1()
        .flex_row()
        .child(
            view()
                .flex_1()
                .child(when(move || state_for_when.active_tab().is_some(), {
                    let state = state.clone();
                    move || match state.active_tab() {
                        Some(tab) => document_surface(tab, state.clone()).into_element(),
                        None => welcome(state.clone()).into_element(),
                    }
                })),
        )
        .child(when(move || state_for_review.review_open.get(), {
            let state = state.clone();
            move || review_panel(state.clone()).into_element()
        }))
        .child(when(
            {
                let state = state.clone();
                move || state.context_open.get()
            },
            {
                let state = state.clone();
                move || context_panel(state.clone()).into_element()
            },
        ))
}

fn recovery_banner(state: AppState) -> impl IntoElement {
    let restore_state = state.clone();
    let discard_state = state.clone();
    let count_state = state.clone();
    view()
        .px_3()
        .py_2()
        .flex_row()
        .items_center()
        .gap_2()
        .bg(move || theme().card)
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            view()
                .flex_col()
                .flex_1()
                .child(text(move || {
                    format!(
                        "{} unsaved document(s) recovered from a previous session",
                        count_state.recovery_records.get().len()
                    )
                }))
                .child(
                    text("Restore keeps the original file untouched until you save.")
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(small_button("Restore all", move || {
            restore_state.restore_all_recovered()
        }))
        .child(small_button("Discard all", move || {
            discard_state.discard_all_recovered()
        }))
}

fn context_panel(state: AppState) -> impl IntoElement {
    let close_state = state.clone();
    let refresh_state = state.clone();
    let review_state = state.clone();
    let count_state = state.clone();
    let rows_state = state.clone();
    let error_state = state.clone();
    view()
        .w(px(420.0))
        .h(relative(1.0))
        .flex_col()
        .bg(move || theme().card)
        .border_l_1()
        .border_color(move || theme().border)
        .child(
            view()
                .px_3()
                .py_2()
                .flex_row()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    text("Effective Agent Context")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(view().flex_1())
                .child(small_button("Refresh", move || {
                    refresh_state.refresh_context()
                }))
                .child(small_button("Close", move || close_state.close_context())),
        )
        .child(
            scroll()
                .flex_1()
                .flex_col()
                .gap_2()
                .p_3()
                .child(when(
                    {
                        let state = state.clone();
                        move || state.context_busy.get()
                    },
                    move || {
                        text("Resolving instructions...")
                            .text_size(px(12.0))
                            .text_color(move || theme().muted_foreground)
                    },
                ))
                .child(when(
                    move || error_state.context_error.get().is_some(),
                    move || {
                        text(move || {
                            error_state
                                .context_error
                                .get()
                                .map(|error| error.to_string())
                                .unwrap_or_default()
                        })
                        .text_size(px(12.0))
                        .text_color(move || theme().destructive)
                    },
                ))
                .child(each(
                    move || {
                        rows_state
                            .context_resolved
                            .get()
                            .map(|context| context.sources)
                            .unwrap_or_default()
                    },
                    |source: &SourceOccurrence| {
                        format!(
                            "{:?}:{:?}:{}",
                            source.origin,
                            source.rule,
                            source.path.display()
                        )
                    },
                    move |source: ReadSignal<SourceOccurrence>, index: ReadSignal<usize>| {
                        let source = source.get();
                        let index = index.get();
                        let press_state = state.clone();
                        let selected_state = state.clone();
                        let open_state = state.clone();
                        let path = source.path.clone();
                        let content_index = source.content_index;
                        let preview_state = state.clone();
                        let selected =
                            move || selected_state.context_selected.get().contains(&index);
                        view()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .rounded(px(4.0))
                            .bg(move || {
                                if selected() {
                                    theme().secondary
                                } else {
                                    theme().background
                                }
                            })
                            .on_press(move || press_state.toggle_context_source(index))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(text(move || if selected() { "☑" } else { "☐" }))
                                    .child(
                                        text(format!("{:?} / {:?}", source.origin, source.rule))
                                            .text_size(px(11.0)),
                                    )
                                    .child(view().flex_1())
                                    .child(small_button("Open", move || {
                                        open_state.open_file(path.clone())
                                    })),
                            )
                            .child(
                                text(source.path.display().to_string())
                                    .text_size(px(11.0))
                                    .text_color(move || theme().muted_foreground),
                            )
                            .child(
                                text(format!(
                                    "{} of {} bytes included{}",
                                    source.included_bytes,
                                    source.raw_bytes,
                                    if source.is_truncated() {
                                        " (truncated)"
                                    } else {
                                        ""
                                    }
                                ))
                                .text_size(px(10.0))
                                .text_color(move || theme().muted_foreground),
                            )
                            .child(
                                text(move || {
                                    preview_state
                                        .context_resolved
                                        .get()
                                        .and_then(|context| {
                                            content_index
                                                .and_then(|index| context.contents.get(index))
                                                .map(|content| {
                                                    let mut preview: String =
                                                        content.text.chars().take(400).collect();
                                                    if content.text.chars().count() > 400 {
                                                        preview.push('…');
                                                    }
                                                    preview
                                                })
                                        })
                                        .unwrap_or_default()
                                })
                                .text_size(px(11.0)),
                            )
                    },
                ))
                .child(
                    text(move || format!("Skills ({})", count_state.harness_skills.get().len()))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(each(
                    {
                        let skills_state = count_state.clone();
                        move || skills_state.harness_skills.get()
                    },
                    |skill: &Skill| skill.entry.clone(),
                    {
                        let rows_state = count_state.clone();
                        move |skill: ReadSignal<Skill>, _| {
                            let skill = skill.get();
                            let open_state = rows_state.clone();
                            let entry = skill.entry.clone();
                            view()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .on_press(move || open_state.open_file(entry.clone()))
                                .child(
                                    text(format!("{} · {:?}", skill.name, skill.origin))
                                        .text_size(px(11.0)),
                                )
                        }
                    },
                ))
                .child(
                    text(move || {
                        format!(
                            "Instruction files ({})",
                            count_state.harness_instructions.get().len()
                        )
                    })
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD),
                )
                .child(each(
                    {
                        let instructions_state = count_state.clone();
                        move || instructions_state.harness_instructions.get()
                    },
                    |instruction: &Instruction| instruction.path.clone(),
                    {
                        let rows_state = count_state.clone();
                        move |instruction: ReadSignal<Instruction>, _| {
                            let instruction = instruction.get();
                            let open_state = rows_state.clone();
                            let path = instruction.path.clone();
                            view()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .on_press(move || open_state.open_file(path.clone()))
                                .child(
                                    text(format!(
                                        "{} · {:?}",
                                        instruction.label(),
                                        instruction.origin
                                    ))
                                    .text_size(px(11.0)),
                                )
                        }
                    },
                ))
                .child(each(
                    move || {
                        count_state
                            .context_resolved
                            .get()
                            .map(|context| context.assumptions)
                            .unwrap_or_default()
                    },
                    |assumption: &String| format!("assumption:{assumption}"),
                    move |assumption: ReadSignal<String>, _| {
                        text(move || format!("Assumption: {}", assumption.get()))
                            .text_size(px(11.0))
                            .text_color(move || theme().muted_foreground)
                    },
                ))
                .child(each(
                    move || {
                        count_state
                            .context_resolved
                            .get()
                            .map(|context| context.diagnostics)
                            .unwrap_or_default()
                    },
                    |diagnostic: &mt_core::Diagnostic| diagnostic.to_string(),
                    move |diagnostic: ReadSignal<mt_core::Diagnostic>, _| {
                        text(move || diagnostic.get().to_string())
                            .text_size(px(11.0))
                            .text_color(move || theme().destructive)
                    },
                ))
                .child(
                    text(move || {
                        count_state
                            .context_resolved
                            .get()
                            .map(|context| {
                                let root = context
                                    .project_root
                                    .as_ref()
                                    .map(|root| root.display().to_string())
                                    .unwrap_or_else(|| "(none)".to_owned());
                                format!(
                                    "Status: {:?} · {} source(s) · project root: {root}",
                                    context.status,
                                    context.sources.len()
                                )
                            })
                            .unwrap_or_default()
                    })
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            view()
                .p_3()
                .border_t_1()
                .border_color(move || theme().border)
                .child(
                    button()
                        .w(relative(1.0))
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .bg(move || theme().primary)
                        .on_press(move || review_state.review_with_context())
                        .child(
                            text("Review with selected context")
                                .text_size(px(12.0))
                                .text_color(move || theme().primary_foreground),
                        ),
                ),
        )
}

fn welcome(state: AppState) -> impl IntoElement {
    let open_state = state.clone();
    let sample_state = state.clone();
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            text("MarkTurbo")
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text("Open a folder, then a Markdown file.")
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(small_button("Open folder", move || {
                    choose_folder(open_state.clone())
                }))
                .child(when(
                    crate::app_paths::bundled_sample_available,
                    move || {
                        let sample_state = sample_state.clone();
                        small_button("Open sample", move || sample_state.open_bundled_sample())
                    },
                )),
        )
        .child(when(
            move || !state.settings.get().recent_targets.is_empty(),
            {
                let state = state.clone();
                move || {
                    let state = state.clone();
                    view()
                        .flex_col()
                        .gap_1()
                        .w(px(520.0))
                        .child(
                            text("Recent")
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(each(
                            move || state.settings.get().recent_targets,
                            |target: &RecentTarget| target.path.clone(),
                            move |target: ReadSignal<RecentTarget>, _| {
                                let target = target.get();
                                let state = state.clone();
                                let kind = match target.kind {
                                    RecentTargetKind::Workspace => "Workspace",
                                    RecentTargetKind::File => "File",
                                };
                                let display_name = target.display_name.clone();
                                let target_for_open = target.clone();
                                view()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .on_press(move || state.open_recent(&target_for_open))
                                    .child(
                                        text(format!("{kind}  {display_name}")).text_size(px(12.0)),
                                    )
                            },
                        ))
                }
            },
        ))
}

fn tab_bar(state: AppState) -> impl IntoElement {
    view()
        .h(px(38.0))
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .border_b_1()
        .border_color(move || theme().border)
        .child(each(move || state.tabs.get(), |tab: &Tab| tab.id, {
            let state = state.clone();
            move |tab: ReadSignal<Tab>, _| {
                let tab = tab.get();
                let id = tab.id;
                let title = tab.title.clone();
                let dirty = tab.dirty;
                let state_row = state.clone();
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded(px(5.0))
                    .bg(move || {
                        if state.active.get() == Some(id) {
                            theme().accent
                        } else {
                            theme().card
                        }
                    })
                    .on_press(move || state_row.set_active(id))
                    .child(text(move || {
                        if dirty.get() {
                            format!("● {title}")
                        } else {
                            title.to_string()
                        }
                    }))
                    .child(
                        text("×")
                            .text_color(move || theme().muted_foreground)
                            .on_press({
                                let state = state.clone();
                                move || state.request_close_tab(id)
                            }),
                    )
            }
        }))
}

fn toolbar(state: AppState) -> impl IntoElement {
    let tab = state.active_tab();
    view()
        .h(px(40.0))
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            button()
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .bg(move || theme().secondary)
                .on_press({
                    let state = state.clone();
                    move || state.save_active()
                })
                .child(text("Save").text_size(px(12.0))),
        )
        .child(
            button()
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .bg(move || theme().secondary)
                .on_press({
                    let state = state.clone();
                    move || state.save_as_active()
                })
                .child(text("Save As").text_size(px(12.0))),
        )
        .child(pane_button(
            state.clone(),
            tab.clone(),
            Pane::Source,
            "Source",
        ))
        .child(pane_button(
            state.clone(),
            tab.clone(),
            Pane::Preview,
            "Preview",
        ))
        .child(pane_button(state.clone(), tab.clone(), Pane::Web, "Web"))
        .child(pane_button(
            state.clone(),
            tab.clone(),
            Pane::Split,
            "Split",
        ))
        .child(pane_button(state.clone(), tab, Pane::SplitWeb, "Split+Web"))
        .child(view().flex_1())
        .child(
            text(move || {
                state
                    .active_tab()
                    .map(|tab| tab.path.display().to_string())
                    .unwrap_or_default()
            })
            .text_size(px(11.0))
            .text_color(move || theme().muted_foreground),
        )
}

fn pane_button(
    state: AppState,
    tab: Option<Tab>,
    pane: Pane,
    label: &'static str,
) -> impl IntoElement {
    let tab_id = tab.as_ref().map(|tab| tab.id);
    let pane_signal = tab.as_ref().map(|tab| tab.pane);
    view()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .on_press(move || {
            if let Some(id) = tab_id {
                state.set_pane(id, pane);
            }
        })
        .child(text(label).text_size(px(12.0)).text_color(move || {
            if pane_signal.is_some_and(|pane_signal| pane_signal.get() == pane) {
                theme().primary
            } else {
                theme().muted_foreground
            }
        }))
}

fn document_surface(tab: Tab, state: AppState) -> impl IntoElement {
    let tab_for_source = tab.clone();
    let tab_for_preview = tab.clone();
    let tab_for_web = tab.clone();
    let state_for_web = state.clone();
    let state_for_source = state.clone();
    let conflict_tab = tab.clone();
    let conflict_state = state.clone();
    view()
        .flex_1()
        .h(relative(1.0))
        .flex_col()
        .child(when(move || conflict_tab.conflict.get(), {
            let reload = conflict_state.clone();
            let overwrite = conflict_state.clone();
            let save_as = conflict_state.clone();
            move || {
                let reload = reload.clone();
                let overwrite = overwrite.clone();
                let save_as = save_as.clone();
                view()
                    .px_3()
                    .py_2()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(move || theme().border)
                    .bg(move || theme().card)
                    .child(small_button("Reload", move || reload.reload_active()))
                    .child(small_button("Overwrite", move || {
                        overwrite.overwrite_active()
                    }))
                    .child(small_button("Save As", move || save_as.save_as_active()))
                    .child(
                        text("Save conflict")
                            .text_size(px(12.0))
                            .text_color(move || theme().destructive),
                    )
            }
        }))
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(when(
                    move || matches!(tab.pane.get(), Pane::Source | Pane::Split | Pane::SplitWeb),
                    move || {
                        source_pane(tab_for_source.clone(), state_for_source.clone()).into_element()
                    },
                ))
                .child(when(
                    move || matches!(tab.pane.get(), Pane::Preview | Pane::Split),
                    move || preview_pane(tab_for_preview.clone()).into_element(),
                ))
                .child(when(
                    move || matches!(tab.pane.get(), Pane::Web | Pane::SplitWeb),
                    move || web_pane(tab_for_web.clone(), state_for_web.clone()).into_element(),
                )),
        )
}

fn source_pane(tab: Tab, state: AppState) -> impl IntoElement {
    let language = SharedString::from(language_for_path(&tab.path));
    editor()
        .value(move || Some(tab.text.get()))
        .language(Some(language))
        .accessibility_label(Some(SharedString::from("Markdown source")))
        .flex_1()
        .h(relative(1.0))
        .on_change(move |value: &SharedString| {
            // nsh reports the bound value once on mount. A document is dirty
            // only after the buffer differs from what the editor already has.
            if tab.text.get().as_str() == value.as_str() {
                return;
            }
            tab.text.set(value.clone());
            tab.dirty.set(true);
            tab.revision.update(|revision| *revision += 1);
            state.note_edit(tab.id);
        })
}

fn web_pane(tab: Tab, state: AppState) -> impl IntoElement {
    let trusted = tab.web_trusted;
    let html = computed(move || {
        let document = Document::new(Some(tab.path.clone()), tab.text.get().to_string());
        let trust = if trusted.get() {
            super::web::Trust::Trusted
        } else {
            super::web::Trust::Restricted
        };
        super::web::build_html(&document, renderer(), trust, dark())
    });
    view()
        .flex_1()
        .h(relative(1.0))
        .flex_col()
        .child(
            view()
                .h(px(30.0))
                .px_2()
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    text(move || {
                        if trusted.get() {
                            "Trusted: scripts may run"
                        } else {
                            "Restricted: scripts disabled"
                        }
                    })
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground),
                )
                .child(when(move || !trusted.get(), {
                    let state = state.clone();
                    let id = tab.id;
                    move || {
                        let state = state.clone();
                        small_button("Trust scripts", move || state.request_web_trust(id))
                    }
                })),
        )
        .child(
            web_view_html(move || html.get(), None)
                .java_script_enabled(move || trusted.get())
                .flex_1(),
        )
}

fn preview_pane(tab: Tab) -> impl IntoElement {
    let preview = computed(move || preprocess_markdown(&tab.text.get(), &tab.path));
    view()
        .flex_1()
        .h(relative(1.0))
        .bg(move || theme().background)
        .child(
            scroll().flex_1().child(
                text_view()
                    .markdown(move || Some(SharedString::from(preview.get())))
                    .selectable(true)
                    .render_math(|math: &TextViewMath| render_diagram("math", math.tex.as_ref()))
                    .p_4(),
            ),
        )
}

fn review_panel(state: AppState) -> impl IntoElement {
    view()
        .w(px(380.0))
        .h(relative(1.0))
        .flex_col()
        .bg(move || theme().card)
        .border_l_1()
        .border_color(move || theme().border)
        .child(
            view()
                .h(px(38.0))
                .px_3()
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(move || theme().border)
                .child(text("Review").font_weight(FontWeight::SEMIBOLD))
                .child(
                    text("×")
                        .text_color(move || theme().muted_foreground)
                        .on_press({
                            let state = state.clone();
                            move || state.close_review()
                        }),
                ),
        )
        .child(
            scroll().flex_1().child(
                view()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(when(
                        move || state.review_busy.get(),
                        || text("Reviewing…").text_color(move || theme().muted_foreground),
                    ))
                    .child(when(move || state.review_consent.get().is_some(), {
                        let state = state.clone();
                        move || {
                            let send = state.clone();
                            let cancel = state.clone();
                            view()
                                .flex_col()
                                .gap_2()
                                .child(
                                    text(state.review_consent.get().unwrap_or_default())
                                        .text_size(px(12.0))
                                        .text_color(move || theme().muted_foreground),
                                )
                                .child(
                                    view()
                                        .flex_row()
                                        .gap_2()
                                        .child(small_button("Send", move || send.send_review()))
                                        .child(small_button("Cancel", move || {
                                            cancel.cancel_review()
                                        })),
                                )
                        }
                    }))
                    .child(when(move || state.review_view.get().is_some(), {
                        let state = state.clone();
                        move || review_result(state.clone())
                    }))
                    .child(when(move || state.revision_busy.get(), {
                        move || text("Revising…").text_color(move || theme().muted_foreground)
                    }))
                    .child(when(move || state.revision_consent.get().is_some(), {
                        let state = state.clone();
                        move || {
                            let send = state.clone();
                            let cancel = state.clone();
                            view()
                                .flex_col()
                                .gap_2()
                                .child(
                                    text(state.revision_consent.get().unwrap_or_default())
                                        .text_size(px(12.0))
                                        .text_color(move || theme().muted_foreground),
                                )
                                .child(
                                    view()
                                        .flex_row()
                                        .gap_2()
                                        .child(small_button("Send revision", move || {
                                            send.send_revision()
                                        }))
                                        .child(small_button("Cancel", move || {
                                            cancel.cancel_revision()
                                        })),
                                )
                        }
                    }))
                    .child(when(move || state.revision_status.get().is_some(), {
                        move || {
                            text(move || state.revision_status.get().unwrap_or_default())
                                .text_size(px(12.0))
                        }
                    }))
                    .child(each(
                        move || state.revision_hunks.get(),
                        |hunk: &crate::state::RevisionHunkView| hunk.ordinal,
                        {
                            let state = state.clone();
                            move |hunk: ReadSignal<crate::state::RevisionHunkView>,
                                  index: ReadSignal<usize>| {
                                let hunk = hunk.get();
                                let index = index.get();
                                let accepted = hunk.accepted;
                                let state_for_press = state.clone();
                                view()
                                    .flex_col()
                                    .gap_1()
                                    .p_2()
                                    .rounded(px(4.0))
                                    .bg(move || {
                                        if accepted.get() {
                                            theme().secondary
                                        } else {
                                            theme().background
                                        }
                                    })
                                    .on_press(move || state_for_press.toggle_revision_hunk(index))
                                    .child(text(move || {
                                        if accepted.get() {
                                            "☑ Approved"
                                        } else {
                                            "☐ Rejected"
                                        }
                                    }))
                                    .child(
                                        text(hunk.rationale)
                                            .text_size(px(11.0))
                                            .text_color(move || theme().muted_foreground),
                                    )
                                    .child(
                                        text(format!("− {}", hunk.before))
                                            .text_size(px(11.0))
                                            .text_color(move || theme().destructive),
                                    )
                                    .child(
                                        text(format!("+ {}", hunk.after))
                                            .text_size(px(11.0))
                                            .text_color(move || theme().primary),
                                    )
                            }
                        },
                    ))
                    .child(when(move || !state.revision_hunks.get().is_empty(), {
                        let state = state.clone();
                        move || {
                            let state = state.clone();
                            small_button("Apply approved", move || state.apply_revision())
                        }
                    })),
            ),
        )
}

fn review_result(state: AppState) -> impl IntoElement {
    view()
        .flex_col()
        .gap_3()
        .child(
            text(move || {
                state
                    .review_view
                    .get()
                    .map(|view| view.status)
                    .unwrap_or_default()
            })
            .font_weight(FontWeight::SEMIBOLD),
        )
        .child(when(
            move || {
                state
                    .review_view
                    .get()
                    .and_then(|view| view.diagnostic)
                    .is_some()
            },
            {
                let state = state.clone();
                move || {
                    text(
                        state
                            .review_view
                            .get()
                            .and_then(|view| view.diagnostic)
                            .unwrap_or_default(),
                    )
                    .text_size(px(12.0))
                    .text_color(move || theme().destructive)
                }
            },
        ))
        .child(each(
            move || {
                state
                    .review_view
                    .get()
                    .map(|view| view.sections)
                    .unwrap_or_default()
            },
            |section: &crate::state::ReviewSection| section.title.clone(),
            |section: ReadSignal<crate::state::ReviewSection>, _| {
                let section = section.get();
                view()
                    .flex_col()
                    .gap_1()
                    .child(text(section.title.clone()).font_weight(FontWeight::SEMIBOLD))
                    .child(each(
                        move || section.items.clone(),
                        |item: &String| item.clone(),
                        |item: ReadSignal<String>, _| {
                            text(item.get())
                                .text_size(px(12.0))
                                .text_color(move || theme().muted_foreground)
                        },
                    ))
            },
        ))
        .child(review_list(state.clone(), "Findings", |view| {
            view.findings.clone()
        }))
        .child(review_list(state.clone(), "Questions", |view| {
            view.questions.clone()
        }))
        .child(each(
            move || state.revision_questions.get(),
            |question: &crate::state::RevisionQuestionView| question.ordinal,
            move |question: ReadSignal<crate::state::RevisionQuestionView>, _| {
                let question = question.get();
                let answer = question.answer;
                view()
                    .flex_col()
                    .gap_1()
                    .child(
                        text(format!("{} · {}", question.priority, question.question))
                            .text_size(px(11.0))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        input()
                            .value(move || answer.get())
                            .on_change(move |value: &str| answer.set(value.to_owned()))
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(move || theme().border),
                    )
            },
        ))
        .child(
            small_button("Prepare revision", {
                let state = state.clone();
                move || state.start_revision()
            })
            .into_element(),
        )
}

fn review_list(
    state: AppState,
    title: &'static str,
    items: fn(&crate::state::ReviewView) -> Vec<String>,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap_1()
        .child(when(
            move || {
                state
                    .review_view
                    .get()
                    .is_some_and(|view| !items(&view).is_empty())
            },
            move || text(title).font_weight(FontWeight::SEMIBOLD),
        ))
        .child(each(
            move || {
                state
                    .review_view
                    .get()
                    .map(|view| items(&view))
                    .unwrap_or_default()
            },
            |item: &String| item.clone(),
            |item: ReadSignal<String>, _| {
                text(item.get())
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
}

fn small_button(label: &'static str, on_press: impl Fn() + 'static) -> impl IntoElement {
    button()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .bg(move || theme().secondary)
        .on_press(on_press)
        .child(text(label).text_size(px(12.0)))
}

fn settings_page(state: AppState) -> impl IntoElement {
    let credential = signal(String::new());
    let notice = signal(SharedString::from(""));
    let close = state.clone();
    let save = state.clone();
    let settings = state.settings.get();
    let provider_state = state.clone();
    let model_state = state.clone();
    let base_state = state.clone();
    let credential_state = state.clone();
    let language_en = state.clone();
    let language_zh = state.clone();
    let theme_system = state.clone();
    let theme_light = state.clone();
    let theme_dark = state.clone();
    view()
        .flex_1()
        .flex_col()
        .gap_4()
        .p_6()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("Settings")
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(small_button("Close", move || close.close_settings())),
        )
        .child(settings_field(
            "Interface language",
            view()
                .flex_row()
                .gap_2()
                .child(choice_button(
                    "English",
                    settings.language == mt_core::settings::Language::English,
                    move || language_en.set_language(mt_core::settings::Language::English),
                ))
                .child(choice_button(
                    "简体中文",
                    settings.language == mt_core::settings::Language::Chinese,
                    move || language_zh.set_language(mt_core::settings::Language::Chinese),
                )),
        ))
        .child(settings_field(
            "Theme",
            view()
                .flex_row()
                .gap_2()
                .child(choice_button(
                    "System",
                    settings.theme == mt_core::settings::ThemePreference::System,
                    move || theme_system.set_theme(mt_core::settings::ThemePreference::System),
                ))
                .child(choice_button(
                    "Light",
                    settings.theme == mt_core::settings::ThemePreference::Light,
                    move || theme_light.set_theme(mt_core::settings::ThemePreference::Light),
                ))
                .child(choice_button(
                    "Dark",
                    settings.theme == mt_core::settings::ThemePreference::Dark,
                    move || theme_dark.set_theme(mt_core::settings::ThemePreference::Dark),
                )),
        ))
        .child(settings_field(
            "Provider",
            settings_input(settings.model_provider.clone(), move |value| {
                provider_state
                    .settings
                    .update(|settings| settings.model_provider = value)
            }),
        ))
        .child(settings_field(
            "Model",
            settings_input(settings.model_name.clone(), move |value| {
                model_state
                    .settings
                    .update(|settings| settings.model_name = value)
            }),
        ))
        .child(settings_field(
            "Base URL",
            settings_input(settings.model_base_url.clone(), move |value| {
                base_state
                    .settings
                    .update(|settings| settings.model_base_url = value)
            }),
        ))
        .child(settings_field(
            "API key (this session only)",
            view()
                .flex_row()
                .gap_2()
                .items_center()
                .child(
                    input()
                        .masked(true)
                        .value(move || credential.get())
                        .on_change(move |value: &str| credential.set(value.to_owned()))
                        .px_2()
                        .py_1()
                        .flex_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(move || theme().border),
                )
                .child(small_button("Use", {
                    let credential_state = credential_state.clone();
                    move || match credential_state.store_session_credential(&credential.get()) {
                        Ok(()) => notice.set(SharedString::from("Key stored for this session.")),
                        Err(error) => notice.set(SharedString::from(error)),
                    }
                }))
                .child(small_button(
                    "Store securely",
                    move || match credential_state.store_persistent_credential(&credential.get()) {
                        Ok(()) => notice.set(SharedString::from("Key stored securely.")),
                        Err(error) => notice.set(SharedString::from(error)),
                    },
                )),
        ))
        .child(
            text(move || notice.get())
                .text_size(px(12.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(small_button("Save", move || save.save_settings()))
}

fn settings_field(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap_1()
        .child(
            text(label)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(control)
}

fn settings_input(value: String, update: impl Fn(String) + 'static) -> impl IntoElement {
    input()
        .value(value)
        .on_change(move |value: &str| update(value.to_owned()))
        .px_2()
        .py_1()
        .w(px(420.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(move || theme().border)
}

fn choice_button(
    label: &'static str,
    active: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    button()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .bg(move || {
            if active {
                theme().primary
            } else {
                theme().secondary
            }
        })
        .on_press(on_press)
        .child(text(label).text_size(px(12.0)).text_color(move || {
            if active {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        }))
}

fn status_bar(state: AppState) -> impl IntoElement {
    view()
        .h(px(24.0))
        .px_2()
        .flex_row()
        .items_center()
        .border_t_1()
        .border_color(move || theme().border)
        .child(
            text(move || state.status.get())
                .test_id("status")
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn choose_folder(state: AppState) {
    let picked = pick_paths(OpenDialog::folders().multiple(false));
    spawn(async move {
        match picked.await {
            Ok(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    state.open_folder(path);
                }
            }
            Ok(None) => {}
            Err(error) => state
                .status
                .set(SharedString::from(format!("Folder picker failed: {error}"))),
        }
    });
}

fn renderer() -> &'static RendererRegistry {
    static REGISTRY: LazyLock<RendererRegistry> = LazyLock::new(RendererRegistry::with_defaults);
    &REGISTRY
}

fn preprocess_markdown(source: &str, path: &std::path::Path) -> String {
    let document = Document::new(Some(path.to_path_buf()), source.to_owned());
    let mut out = source.to_owned();
    for block in document.blocks().iter().rev() {
        let BlockKind::Diagram(kind) = &block.kind else {
            continue;
        };
        let RenderOutcome::Svg(markup) = renderer().render(kind.id(), &block.content) else {
            continue;
        };
        let replacement = format!(
            "\n<img src=\"{}\" alt=\"{}\">\n",
            svg_data_uri(&markup),
            kind
        );
        out.replace_range(block.range.clone(), &replacement);
    }
    out
}

fn svg_data_uri(markup: &str) -> String {
    use std::fmt::Write;
    let mut uri = String::with_capacity(markup.len() + 32);
    uri.push_str("data:image/svg+xml,");
    for byte in markup.bytes() {
        if byte.is_ascii_alphanumeric() || b" -._~!$'()*+,;=:@/?".contains(&byte) {
            uri.push(byte as char);
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

fn render_diagram(kind: &str, source: &str) -> AnyElement {
    match renderer().render(kind, source) {
        RenderOutcome::Svg(markup) => svg(ImageSrc::svg_markup(&markup)).into_element(),
        RenderOutcome::Failed(_diagnostic) => text(source.to_owned())
            .font_family("monospace")
            .text_size(px(12.0))
            .text_color(move || theme().muted_foreground)
            .into_element(),
    }
}
