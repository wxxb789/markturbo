//! Application root and the window's three regions: workspace, document and
//! status. Components run once; every changing value is a signal.

use std::{cell::Cell, path::PathBuf, rc::Rc, sync::LazyLock, time::Duration};

use mt_core::{
    BlockKind, Document,
    agent_artifacts::{context::SourceOccurrence, instruction::Instruction, skill::Skill},
    rendering::{RenderOutcome, RendererRegistry},
    settings::{Language, RecentTarget, RecentTargetKind},
};
use nsh_base::{TextViewMath, button, editor, input, text_view, theme};
use nsh_core::{element::ImageSrc, file_dialog::OpenDialog, prelude::*};

use crate::i18n::{self, Key};
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
    let shell_state = state.clone();
    view()
        .flex_1()
        .flex_row()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(each(
            move || {
                vec![(
                    shell_state.settings.get().language,
                    window().width.0 < 900.0,
                )]
            },
            |key: &(Language, bool)| (language_code(key.0), key.1),
            {
                let state = state.clone();
                move |key: ReadSignal<(Language, bool)>, _| {
                    let (language, compact) = key.get();
                    shell(state.clone(), language, compact).into_element()
                }
            },
        ))
}

fn language_code(language: Language) -> u8 {
    match language {
        Language::English => 0,
        Language::Chinese => 1,
    }
}

fn shell(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .child(sidebar(state.clone(), language, compact))
        .child(main(state, language, compact))
}

/// Marks the checkpoint ticker inactive when the window's owner is disposed,
/// so a racing interval cannot read signals that are already gone.
struct TickerGuard(Rc<Cell<bool>>);

impl Drop for TickerGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn sidebar(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    view()
        .w(px(if compact { 200.0 } else { 260.0 }))
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
                    text(i18n::t(language, Key::Workspace))
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
                        .child(text(i18n::t(language, Key::Open)).text_size(px(12.0))),
                ),
        )
        .child(search_row(state.clone(), language))
        .child(
            scroll().flex_1().child(
                view()
                    .flex_col()
                    .child(when(move || state.search_ran.get(), {
                        let state = state.clone();
                        move || search_results_view(state.clone(), language).into_element()
                    }))
                    .child(when(move || !state.search_ran.get(), {
                        let state = state.clone();
                        move || tree_view(state.clone()).into_element()
                    })),
            ),
        )
        .child(
            view()
                .p_2()
                .border_t_1()
                .border_color(move || theme().border)
                .flex_col()
                .gap_1()
                .child(small_button(i18n::t(language, Key::AgentContext), {
                    let state = state.clone();
                    move || state.toggle_context()
                }))
                .child(small_button(i18n::t(language, Key::Settings), {
                    let state = state.clone();
                    move || state.open_settings()
                }))
                .child(small_button(i18n::t(language, Key::Quit), {
                    let state = state.clone();
                    move || state.request_quit()
                })),
        )
}

fn search_row(state: AppState, language: Language) -> impl IntoElement {
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
                .placeholder(i18n::t(language, Key::SearchFiles))
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
                .child(small_button(
                    i18n::t(language, Key::PanelSearch),
                    move || go_state.run_search(&go_state.search_query.get()),
                ))
                .child(small_button(i18n::t(language, Key::Clear), {
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

fn search_results_view(state: AppState, language: Language) -> impl IntoElement {
    let results = state.search_results;
    view()
        .flex_col()
        .child(when(
            move || state.search_busy.get(),
            move || {
                text(i18n::t(language, Key::Searching))
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(when(
            move || state.search_truncated.get(),
            move || {
                text(i18n::t(language, Key::ShowingFirstResults))
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(when(
            move || !state.search_busy.get() && results.get().is_empty(),
            move || {
                text(i18n::t(language, Key::NoMatches))
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

fn main(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let state_for_settings = state.clone();
    let state_for_toolbar = state.clone();
    let state_for_tabs = state.clone();
    view()
        .flex_1()
        .h(relative(1.0))
        .flex_col()
        .child(header(state.clone(), language))
        .child(when(move || !state_for_tabs.tabs.get().is_empty(), {
            let state = state.clone();
            move || tab_bar(state.clone()).into_element()
        }))
        .child(when(move || state_for_toolbar.active_tab().is_some(), {
            let state = state.clone();
            move || toolbar(state.clone(), language, compact).into_element()
        }))
        .child(when(
            {
                let state = state.clone();
                move || !state.recovery_records.get().is_empty()
            },
            {
                let state = state.clone();
                move || recovery_banner(state.clone(), language).into_element()
            },
        ))
        .child(
            when(move || state_for_settings.settings_open.get(), {
                let state = state.clone();
                move || settings_page(state.clone(), language, compact).into_element()
            })
            .otherwise({
                let state = state.clone();
                move || document_area(state.clone(), language, compact).into_element()
            }),
        )
        .child(status_bar(state, language))
}

fn header(state: AppState, language: Language) -> impl IntoElement {
    let press_state = state.clone();
    view()
        .h(px(48.0))
        .px_3()
        .flex_row()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            text("MarkTurbo")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(view().flex_1())
        .child(
            button()
                .px_3()
                .py_1()
                .rounded(px(6.0))
                .bg(move || theme().primary)
                .disabled(move || state.active_tab().is_none())
                .on_press(move || press_state.start_review())
                .child(
                    text(i18n::t(language, Key::Review))
                        .text_size(px(12.0))
                        .text_color(move || theme().primary_foreground),
                ),
        )
}

fn document_area(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let state_for_empty = state.clone();
    let state_for_review = state.clone();
    view()
        .flex_1()
        .flex_row()
        .child(
            view()
                .flex_1()
                .child(when(move || state_for_empty.active_tab().is_none(), {
                    let state = state.clone();
                    move || welcome(state.clone(), language, compact).into_element()
                }))
                .child(each(
                    {
                        let state = state.clone();
                        move || state.active_tab().into_iter().collect::<Vec<_>>()
                    },
                    |tab: &Tab| tab.id,
                    {
                        let state = state.clone();
                        move |tab: ReadSignal<Tab>, _| {
                            document_surface(tab.get(), state.clone(), language).into_element()
                        }
                    },
                )),
        )
        .child(when(move || state_for_review.review_open.get(), {
            let state = state.clone();
            move || review_panel(state.clone(), language, compact).into_element()
        }))
        .child(when(
            {
                let state = state.clone();
                move || state.context_open.get()
            },
            {
                let state = state.clone();
                move || context_panel(state.clone(), language, compact).into_element()
            },
        ))
}

fn recovery_banner(state: AppState, language: Language) -> impl IntoElement {
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
                        "{} ({})",
                        i18n::t(language, Key::RestoredDocuments),
                        count_state.recovery_records.get().len()
                    )
                }))
                .child(
                    text(i18n::t(language, Key::RestoreKeepsSource))
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(small_button(
            i18n::t(language, Key::RestoreAll),
            move || restore_state.restore_all_recovered(),
        ))
        .child(small_button(
            i18n::t(language, Key::DiscardAll),
            move || discard_state.discard_all_recovered(),
        ))
}

fn context_panel(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let close_state = state.clone();
    let refresh_state = state.clone();
    let review_state = state.clone();
    let count_state = state.clone();
    let rows_state = state.clone();
    let error_state = state.clone();
    view()
        .w(px(if compact { 320.0 } else { 420.0 }))
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
                    text(i18n::t(language, Key::EffectiveAgentContext))
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(view().flex_1())
                .child(small_button(i18n::t(language, Key::Refresh), move || {
                    refresh_state.refresh_context()
                }))
                .child(small_button(i18n::t(language, Key::Close), move || {
                    close_state.close_context()
                })),
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
                        text(i18n::t(language, Key::ResolvingInstructions))
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
                                    .child(small_button(i18n::t(language, Key::Open), move || {
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
                    text(move || {
                        format!(
                            "{} ({})",
                            i18n::t(language, Key::SectionSkills),
                            count_state.harness_skills.get().len()
                        )
                    })
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
                            "{} ({})",
                            i18n::t(language, Key::Instructions),
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
                        text(move || {
                            format!(
                                "{}: {}",
                                i18n::t(language, Key::Assumption),
                                assumption.get()
                            )
                        })
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
                            text(i18n::t(language, Key::ReviewWithContext))
                                .text_size(px(12.0))
                                .text_color(move || theme().primary_foreground),
                        ),
                ),
        )
}

fn welcome(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let open_state = state.clone();
    let sample_state = state.clone();
    let new_state = state.clone();
    let paste_state = state.clone();
    view()
        .test_id("welcome")
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            text("MarkTurbo")
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(i18n::t(language, Key::WelcomeTitle))
                .text_size(px(24.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(i18n::t(language, Key::WelcomeSubtitle))
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(small_button(
                    i18n::t(language, Key::NewDocument),
                    move || new_state.new_document(),
                ))
                .child(small_button(i18n::t(language, Key::Paste), move || {
                    paste_state.new_from_clipboard()
                }))
                .child(
                    button()
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .bg(move || theme().primary)
                        .on_press(move || choose_folder(open_state.clone()))
                        .child(
                            text(i18n::t(language, Key::OpenFolder))
                                .text_size(px(12.0))
                                .text_color(move || theme().primary_foreground),
                        ),
                )
                .child(when(
                    crate::app_paths::bundled_sample_available,
                    move || {
                        let sample_state = sample_state.clone();
                        small_button(i18n::t(language, Key::OpenSample), move || {
                            sample_state.open_bundled_sample()
                        })
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
                        .w(px(if compact { 360.0 } else { 520.0 }))
                        .child(
                            text(i18n::t(language, Key::Recent))
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
                                    RecentTargetKind::Workspace => {
                                        i18n::t(language, Key::Workspace)
                                    }
                                    RecentTargetKind::File => i18n::t(language, Key::File),
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

fn toolbar(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let more = signal(false);
    let save_state = state.clone();
    let save_as_state = state.clone();
    let translate_state = state.clone();
    let path_state = state.clone();
    let advanced_state = state.clone();
    let more_for_close = more;
    let more_for_open = more;
    view()
        .flex_col()
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            view()
                .h(px(40.0))
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .child(small_button(i18n::t(language, Key::Save), move || {
                    save_state.save_active()
                }))
                .child(pane_button(
                    state.clone(),
                    Pane::Source,
                    i18n::t(language, Key::ModeSource),
                ))
                .child(pane_button(
                    state.clone(),
                    Pane::Preview,
                    i18n::t(language, Key::ModeNative),
                ))
                .child(when(move || more_for_close.get(), {
                    let more = more_for_close;
                    move || {
                        small_button(i18n::t(language, Key::FewerLayouts), move || {
                            more.set(false)
                        })
                    }
                }))
                .child(when(move || !more_for_open.get(), {
                    let more = more_for_open;
                    move || {
                        small_button(i18n::t(language, Key::MoreLayouts), move || more.set(true))
                    }
                }))
                .child(view().flex_1())
                .child(when(move || !compact, {
                    move || {
                        let path_state = path_state.clone();
                        text(move || {
                            path_state
                                .active_tab()
                                .map(|tab| tab.path.display().to_string())
                                .unwrap_or_default()
                        })
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground)
                    }
                })),
        )
        .child(when(move || more.get(), {
            let state = advanced_state;
            move || {
                let save_as = save_as_state.clone();
                let translate = translate_state.clone();
                view()
                    .px_2()
                    .pb_2()
                    .flex_col()
                    .gap_1()
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(small_button(
                                i18n::t(language, Key::SaveAsPicker),
                                move || save_as.save_as_active(),
                            ))
                            .child(small_button(i18n::t(language, Key::Translate), move || {
                                translate.translate_active()
                            })),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(pane_button(
                                state.clone(),
                                Pane::Web,
                                i18n::t(language, Key::ModeWeb),
                            ))
                            .child(pane_button(
                                state.clone(),
                                Pane::Split,
                                i18n::t(language, Key::ModeSplitNative),
                            ))
                            .child(pane_button(
                                state.clone(),
                                Pane::SplitWeb,
                                i18n::t(language, Key::ModeSplitWeb),
                            )),
                    )
            }
        }))
}

fn pane_button(state: AppState, pane: Pane, label: &'static str) -> impl IntoElement {
    small_button(label, move || {
        if let Some(tab) = state.active_tab() {
            state.set_pane(tab.id, pane);
        }
    })
}

fn document_surface(tab: Tab, state: AppState, language: Language) -> impl IntoElement {
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
                    .child(small_button(
                        i18n::t(language, Key::ReloadFromDisk),
                        move || reload.reload_active(),
                    ))
                    .child(small_button(i18n::t(language, Key::Overwrite), move || {
                        overwrite.overwrite_active()
                    }))
                    .child(small_button(
                        i18n::t(language, Key::SaveAsPicker),
                        move || save_as.save_as_active(),
                    ))
                    .child(
                        text(i18n::t(language, Key::SaveConflict))
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
                    move || {
                        web_pane(tab_for_web.clone(), state_for_web.clone(), language)
                            .into_element()
                    },
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

fn web_pane(tab: Tab, state: AppState, language: Language) -> impl IntoElement {
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
                            i18n::t(language, Key::ScriptsTrusted)
                        } else {
                            i18n::t(language, Key::ScriptsRestricted)
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
                        small_button(i18n::t(language, Key::TrustScripts), move || {
                            state.request_web_trust(id)
                        })
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

fn review_panel(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    view()
        .w(px(if compact { 320.0 } else { 380.0 }))
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
                .child(text(i18n::t(language, Key::Review)).font_weight(FontWeight::SEMIBOLD))
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
                        move || {
                            text(i18n::t(language, Key::Reviewing))
                                .text_color(move || theme().muted_foreground)
                        },
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
                                        .child(small_button(
                                            i18n::t(language, Key::SendToModel),
                                            move || send.send_review(),
                                        ))
                                        .child(small_button(
                                            i18n::t(language, Key::Cancel),
                                            move || cancel.cancel_review(),
                                        )),
                                )
                        }
                    }))
                    .child(when(move || state.review_view.get().is_some(), {
                        let state = state.clone();
                        move || review_result(state.clone(), language)
                    }))
                    .child(when(move || state.revision_busy.get(), {
                        move || {
                            text(i18n::t(language, Key::Revising))
                                .text_color(move || theme().muted_foreground)
                        }
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
                                        .child(small_button(
                                            i18n::t(language, Key::SendRevision),
                                            move || send.send_revision(),
                                        ))
                                        .child(small_button(
                                            i18n::t(language, Key::Cancel),
                                            move || cancel.cancel_revision(),
                                        )),
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
                                            format!("☑ {}", i18n::t(language, Key::Approved))
                                        } else {
                                            format!("☐ {}", i18n::t(language, Key::Rejected))
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
                            small_button(i18n::t(language, Key::ApplyApproved), move || {
                                state.apply_revision()
                            })
                        }
                    })),
            ),
        )
}

fn review_result(state: AppState, language: Language) -> impl IntoElement {
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
        .child(review_list(
            state.clone(),
            language,
            Key::ReviewFindings,
            |view| view.findings.clone(),
        ))
        .child(review_list(
            state.clone(),
            language,
            Key::ReviewQuestions,
            |view| view.questions.clone(),
        ))
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
            small_button(i18n::t(language, Key::PrepareRevision), {
                let state = state.clone();
                move || state.start_revision()
            })
            .into_element(),
        )
}

fn review_list(
    state: AppState,
    language: Language,
    title: Key,
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
            move || text(i18n::t(language, title)).font_weight(FontWeight::SEMIBOLD),
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

fn settings_page(state: AppState, language: Language, compact: bool) -> impl IntoElement {
    let credential = signal(String::new());
    let notice = signal(SharedString::from(""));
    let close = state.clone();
    let save = state.clone();
    let settings = state.settings.get();
    let provider_state = state.clone();
    let model_state = state.clone();
    let base_state = state.clone();
    let translate_state = state.clone();
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
        .p(px(if compact { 16.0 } else { 24.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text(i18n::t(language, Key::Settings))
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(small_button(i18n::t(language, Key::Close), move || {
                    close.close_settings()
                })),
        )
        .child(settings_field(
            i18n::t(language, Key::Language_),
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
            i18n::t(language, Key::Theme),
            view()
                .flex_row()
                .gap_2()
                .child(choice_button(
                    i18n::t(language, Key::System),
                    settings.theme == mt_core::settings::ThemePreference::System,
                    move || theme_system.set_theme(mt_core::settings::ThemePreference::System),
                ))
                .child(choice_button(
                    i18n::t(language, Key::Light),
                    settings.theme == mt_core::settings::ThemePreference::Light,
                    move || theme_light.set_theme(mt_core::settings::ThemePreference::Light),
                ))
                .child(choice_button(
                    i18n::t(language, Key::Dark),
                    settings.theme == mt_core::settings::ThemePreference::Dark,
                    move || theme_dark.set_theme(mt_core::settings::ThemePreference::Dark),
                )),
        ))
        .child(settings_field(
            i18n::t(language, Key::Provider),
            settings_input(settings.model_provider.clone(), move |value| {
                provider_state
                    .settings
                    .update(|settings| settings.model_provider = value)
            }),
        ))
        .child(settings_field(
            i18n::t(language, Key::Model),
            settings_input(settings.model_name.clone(), move |value| {
                model_state
                    .settings
                    .update(|settings| settings.model_name = value)
            }),
        ))
        .child(settings_field(
            i18n::t(language, Key::BaseUrl),
            settings_input(settings.model_base_url.clone(), move |value| {
                base_state
                    .settings
                    .update(|settings| settings.model_base_url = value)
            }),
        ))
        .child(settings_field(
            i18n::t(language, Key::TargetLanguage),
            settings_input(settings.translate_to.clone(), move |value| {
                translate_state
                    .settings
                    .update(|settings| settings.translate_to = value)
            }),
        ))
        .child(settings_field(
            i18n::t(language, Key::ApiKey),
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
                .child(small_button(i18n::t(language, Key::Use), {
                    let credential_state = credential_state.clone();
                    move || match credential_state.store_session_credential(&credential.get()) {
                        Ok(()) => notice.set(SharedString::from(i18n::t(
                            language,
                            Key::KeyStoredForSession,
                        ))),
                        Err(error) => notice.set(SharedString::from(error)),
                    }
                }))
                .child(small_button(
                    i18n::t(language, Key::StoreCredentialSecurely),
                    move || match credential_state.store_persistent_credential(&credential.get()) {
                        Ok(()) => notice.set(SharedString::from(i18n::t(
                            language,
                            Key::KeyStoredSecurely,
                        ))),
                        Err(error) => notice.set(SharedString::from(error)),
                    },
                )),
        ))
        .child(
            text(move || notice.get())
                .text_size(px(12.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(small_button(
            i18n::t(language, Key::SaveSettings),
            move || save.save_settings(),
        ))
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

fn status_bar(state: AppState, _language: Language) -> impl IntoElement {
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
