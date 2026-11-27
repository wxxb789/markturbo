//! Owner lane: foundation.
//!
//! The window shell owns the opaque title row and native caption clearance,
//! the sidebar and two-screen stage below it, and the window's overlay layer.
//! Its client width sets the breakpoints (≥1280, 1040–1279, <1040). The stage
//! width is the client width minus the sidebar's drawn width (44–240 points).
//! The document track divides the stage between the mounted source and the
//! preview; the 16-point divider snaps to Edit, Split, or Preview.
//! Review, Revision and Context keep the measured column and the field's
//! 40/32-point insets. Below 1040 the field covers the right 46% and dims the
//! column beneath it. The shell routes these surfaces without drawing their
//! contents.

use std::{cell::Cell, rc::Rc, time::Duration};

use mt_core::settings::Language;
use nsh_core::{Points, TouchEvent, TouchPhase, prelude::*};

use crate::{
    state::{AppState, View as DocumentView},
    ui::{
        consent, context, document, drop_screen, motion, notices, preview, rail, review, revision,
        settings, titlebar,
        tokens::{self, Layout},
        welcome,
    },
};

/// Which surface the field is showing. Exactly one at a time; the order below
/// is the order of precedence, and a revision outranks the review that
/// produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSurface {
    /// Nothing: the room is calm and the field is open space.
    None,
    /// A proposal to accept or reject (revision).
    Revision,
    /// A review: loading, questions, findings, stale, diagnostics.
    Review,
    /// The Effective Agent Context.
    Context,
    /// The rendered document, in Split or Preview.
    Preview,
}

/// What the field shows for `state`.
pub fn surface(state: &AppState) -> FieldSurface {
    if revision::is_open(state) {
        FieldSurface::Revision
    } else if review::is_open(state) {
        FieldSurface::Review
    } else if context::is_open(state) {
        FieldSurface::Context
    } else if preview::is_open(state) {
        FieldSurface::Preview
    } else {
        FieldSurface::None
    }
}

/// The window: title row, sidebar, document stage and overlay layer.
///
/// `layout` is the breakpoint `app.rs` re-keyed the shell with, so a shell that
/// is rebuilt is a shell that crosses one.
pub fn shell(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
    let status = notices::StatusPresence::new(state.clone());
    shell_with_status(state, language, layout, status)
}

pub(crate) fn shell_with_status(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
) -> impl IntoElement {
    // Settings mounts in place of the rail, so the shell owns clearing the
    // narrow overlay when a breakpoint changes with Settings still open.
    if !layout.field_overlays() {
        state.sidebar_overlay.set(false);
    }
    let body_state = state.clone();
    let capture_state = state.clone();
    let drag_state = state.clone();
    let leave_state = state.clone();
    let drop_state = state.clone();
    view()
        .test_id("shell")
        .flex_1()
        .h(relative(1.0))
        .w_full()
        .flex_col()
        // While a consent card is open it is the topmost layer: Enter and
        // Space belong to it even if the focus wandered to a control behind
        // it, so a stray key never presses the dimmed room.
        .capture_keys(move || {
            if consent::is_open(&capture_state) {
                consent::key_capture()
            } else {
                nsh_core::KeyCapture::None
            }
        })
        .on_layout(|frame: &Bounds<Points>| {
            // The shell measures itself: `window()` keeps the size the window
            // was opened with until the next resize (Windows), so a harness
            // that sizes the window before the first frame would otherwise get
            // the wrong breakpoint.
            client_width().set(frame.size.width.0);
        })
        // A drag anywhere over the window offers what its names confirm; the
        // screen leaves on leave or drop. A platform that sends no enter
        // position still gets the screen, which always comes from the right.
        .on_file_drag_enter(move |event: &FileDragEvent| {
            drag_state.drop_hover(&event.uris, &event.mime_types);
        })
        .on_file_drag_leave(move |_: &()| leave_state.drop_leave())
        .on_file_drop(move |event: &FileDropEvent| {
            drop_state.drop_files(&event.uris);
        })
        .bg(move || tokens::palette().room)
        .text_color(move || tokens::palette().ink)
        .font_family(tokens::FONT_UI)
        .text_size(px(tokens::TYPE_UI))
        .line_height(px(tokens::TYPE_UI_LINE))
        .child(titlebar::title_row(state.clone(), language))
        .child(
            when(
                {
                    let state = body_state.clone();
                    move || settings::is_open(&state)
                },
                {
                    let state = body_state.clone();
                    move || settings::page(state.clone(), language, status).into_element()
                },
            )
            .otherwise({
                let state = body_state;
                move || workspace(state.clone(), language, layout, status).into_element()
            }),
        )
        .child(overlay_layer(state, language))
}

/// The shell's own width in points, as laid out. `app.rs` re-keys the shell
/// with [`Layout::of`] of it, so it is the breakpoints' one source of truth.
pub fn client_width() -> Signal<f32> {
    global::<f32>()
}

/// The client width to lay out for: what the shell last measured, or — before
/// the first layout, and on platforms that report it live — what `window()`
/// says. Reading it subscribes.
pub fn client_width_now() -> f32 {
    let measured = client_width().get();
    if measured > 0.0 {
        measured
    } else {
        window().width.0
    }
}

/// The sidebar's drawn push width, measured in client points. Narrow always
/// publishes the strip's width: its opened panel is an overlay, not a push.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SidebarWidth(pub(super) f32);

impl Default for SidebarWidth {
    fn default() -> Self {
        Self(tokens::RAIL_W)
    }
}

pub(super) fn sidebar_width() -> Signal<SidebarWidth> {
    global::<SidebarWidth>()
}

pub(super) fn sidebar_width_now() -> f32 {
    sidebar_width().get().0
}

pub(super) fn stage_width() -> f32 {
    (client_width_now() - sidebar_width_now()).max(0.0)
}

/// The room under the title row: the sidebar and the stage it pushes.
fn workspace(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
) -> impl IntoElement {
    view()
        .test_id("workspace")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .flex_row()
        .child(rail::rail(state.clone(), language, layout))
        .child(stage(state, language, layout, status))
}

fn preview_track(state: &AppState, layout: Layout) -> bool {
    !layout.field_overlays()
        && state.active.get().is_some()
        && matches!(surface(state), FieldSurface::None | FieldSurface::Preview)
}

pub(super) fn field_overlays_source(state: &AppState) -> bool {
    match surface(state) {
        FieldSurface::None => false,
        FieldSurface::Preview => state.active_view() == DocumentView::Split,
        _ => true,
    }
}

fn source_width(state: &AppState, layout: Layout) -> f32 {
    if preview_track(state, layout) {
        stage_width() * state.view_track.get()
    } else if layout.field_overlays() || surface(state) == FieldSurface::None {
        layout.measure() + 2.0 * tokens::COLUMN_GAP
    } else {
        // The dock's own budget: at 1068 client points with the rail folded
        // it is FIELD_MIN_W wide past its right inset. A narrower stage
        // gives the deficit to the source instead of clipping the field.
        (stage_width() - tokens::FIELD_MIN_W - tokens::FIELD_RIGHT_INSET)
            .min(layout.measure() + 2.0 * tokens::COLUMN_GAP)
            .max(0.0)
    }
}

pub(crate) fn shows_full_preview(state: &AppState) -> bool {
    surface(state) == FieldSurface::Preview && state.active_view() == DocumentView::Preview
}

// Move the banners and notice line to the reading screen only once it owns
// the whole document area. The source and its editor stay mounted.
fn preview_owns_chrome(state: &AppState, layout: Layout) -> bool {
    shows_full_preview(state) && (layout.field_overlays() || state.view_track.get() == 0.0)
}

fn stage(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
) -> impl IntoElement {
    let dragging = signal(false);
    let layout_state = state.clone();
    let source_width = computed(move || source_width(&layout_state, layout));
    // Both screens use the same owner decision, including during the last
    // frame of the slide. A clipped source must not retain banner controls.
    let dock_chrome = computed({
        let state = state.clone();
        move || preview_owns_chrome(&state, layout)
    });
    let source_state = state.clone();
    let source_visible = computed(move || {
        if layout.field_overlays() {
            !shows_full_preview(&source_state)
        } else {
            source_width.get() > 0.0
        }
    });
    let overlay_open = state.sidebar_overlay;
    let divider_state = state.clone();
    let source_fade = screen_fade(&state, |state| (!shows_full_preview(state)).then_some(()));
    let dock_fade = screen_fade(&state, |state| match surface(state) {
        FieldSurface::None => None,
        FieldSurface::Preview => state.active_tab().map(|tab| {
            let pane = tab.pane.get();
            (
                FieldSurface::Preview,
                pane.view() == DocumentView::Preview,
                pane.web(),
            )
        }),
        field => Some((field, false, false)),
    });
    let column_state = state.clone();
    view()
        .test_id("stage")
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .h(relative(1.0))
        .overflow_hidden()
        .bg(move || tokens::palette().column_ground)
        .opacity(move || {
            if layout.field_overlays() && overlay_open.get() {
                Layout::DIMMED
            } else {
                1.0
            }
        })
        .on_touch(divider_touch(state.clone(), layout, dragging))
        .child(
            view()
                .test_id("stage-source")
                .w(move || px(source_width.get()))
                .h(relative(1.0))
                .overflow_hidden()
                .pointer_events(move || surface_pointer(source_visible.get()))
                .bind_opacity(&source_fade)
                .child(
                    view()
                        .w_full()
                        .h(relative(1.0))
                        .opacity(move || {
                            if !source_visible.get() {
                                0.0
                            } else if layout.field_overlays()
                                && field_overlays_source(&column_state)
                            {
                                Layout::DIMMED
                            } else {
                                1.0
                            }
                        })
                        .child(column(state.clone(), language, layout, status, dock_chrome)),
                ),
        )
        .child(dock(
            state.clone(),
            language,
            layout,
            source_width,
            dock_fade,
            status,
            dock_chrome,
        ))
        .child(when(
            move || preview_track(&divider_state, layout),
            move || divider(source_width, dragging, language),
        ))
}

/// A pointer owns the free ratio only while the same tab and its track are
/// available. The stage can disappear before that pointer reports its end.
fn divider_touch(state: AppState, layout: Layout, dragging: Signal<bool>) -> impl Fn(&TouchEvent) {
    let captured = Rc::new(Cell::new(None::<(u64, u32, f32)>));
    let finish = {
        let captured = captured.clone();
        let state = state.clone();
        move || {
            let Some((tab, _, _)) = captured.take() else {
                return;
            };
            if untracked(|| state.active.try_get()) == Some(Some(tab)) {
                untracked(|| state.snap_view_track());
            }
        }
    };
    cleanup(finish.clone());
    effect({
        let state = state.clone();
        let captured = captured.clone();
        let finish = finish.clone();
        move || {
            let eligible = preview_track(&state, layout);
            let active = state.active.get();
            if let Some((tab, _, _)) = captured.get()
                && (!eligible || active != Some(tab))
            {
                finish();
                dragging.set(false);
            }
        }
    });
    move |event| {
        let active = state.active.get_untracked();
        if !untracked(|| preview_track(&state, layout))
            || captured
                .get()
                .is_some_and(|(tab, _, _)| Some(tab) != active)
        {
            finish();
            dragging.set(false);
            return;
        }
        let Some((_, pointer, offset)) = captured.get() else {
            if event.phase == TouchPhase::Start
                && let Some(tab) = active
            {
                let ratio = state.view_track.get_untracked();
                let x = untracked(sidebar_width_now) + ratio * untracked(stage_width);
                if let Some(point) = event.points.iter().find(|point| {
                    point.changed && (point.page.x.0 - x).abs() <= tokens::DIVIDER_HIT_W / 2.0
                }) {
                    captured.set(Some((tab, point.id, point.page.x.0 - x)));
                    dragging.set(true);
                    state.set_view_track_now(ratio);
                }
            }
            return;
        };
        let Some(point) = event
            .points
            .iter()
            .find(|point| point.changed && point.id == pointer)
        else {
            return;
        };
        match event.phase {
            TouchPhase::Move => {
                let width = untracked(stage_width);
                let from = untracked(sidebar_width_now);
                state.set_view_track_now((point.page.x.0 - offset - from) / width.max(1.0));
            }
            TouchPhase::End | TouchPhase::Cancel => {
                finish();
                dragging.set(false);
                state.focus_editor();
            }
            TouchPhase::Start => {}
        }
    }
}

/// Spatial motion reveals each pane. In reduced motion the track snaps and
/// a change of the screen actually shown fades in at the destination.
fn screen_fade<T: Copy + PartialEq + 'static>(
    state: &AppState,
    screen: impl Fn(&AppState) -> Option<T> + 'static,
) -> AnimatedValue {
    let opacity = animated_value(1.0);
    let state = state.clone();
    let before = Cell::new(None);
    effect(move || {
        let id = state.active.get();
        let shown = screen(&state);
        let reduced = motion::reduced_motion();
        let previous = before.replace(Some((id, shown, reduced)));
        if previous == Some((id, shown, reduced)) {
            return;
        }
        opacity.set(1.0);
        if shown.is_some()
            && reduced
            && previous.is_some_and(|(tab, screen, was_reduced)| {
                tab == id && was_reduced && screen != shown
            })
        {
            opacity.set(0.0);
            opacity.timing(1.0, motion::REDUCED_CROSSFADE_MS, motion::EASE_OUT);
        }
    });
    opacity
}

fn divider(
    source_width: Memo<f32>,
    dragging: Signal<bool>,
    language: Language,
) -> impl IntoElement {
    let hover = signal(false);
    let strong = move || dragging.get() || hover.get();
    view()
        .test_id("stage-divider")
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(move || px(source_width.get()))
        .ml(px(-tokens::DIVIDER_HIT_W / 2.0))
        .w(px(tokens::DIVIDER_HIT_W))
        .z_index(2)
        .role(AccessibilityRole::Button)
        .label(match language {
            Language::English => "Resize the panes",
            Language::Chinese => "调整窗格大小",
        })
        .cursor(Cursor::ColResize)
        .flex_row()
        .items_center()
        .justify_center()
        .on_mouse_enter(move |_: &MouseEvent| hover.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hover.set(false))
        .child(view().w(px(tokens::HAIRLINE)).h_full().bg(move || {
            if strong() {
                tokens::palette().ink_faint
            } else {
                tokens::palette().hairline
            }
        }))
        .child(
            view()
                .absolute()
                .left(px((tokens::DIVIDER_HIT_W - tokens::DIVIDER_HANDLE_W) / 2.0))
                .w(px(tokens::DIVIDER_HANDLE_W))
                .h(move || {
                    px(if strong() {
                        tokens::SPACE_5
                    } else {
                        tokens::DIVIDER_HANDLE_H
                    })
                })
                .bg(move || {
                    if strong() {
                        tokens::palette().ink
                    } else {
                        tokens::palette().ink_faint
                    }
                }),
        )
}

/// The text column: the document's ground, its measure, its two 40px insets,
/// the banners that belong above the text and the status line at its foot.
///
fn column(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
    dock_chrome: Memo<bool>,
) -> impl IntoElement {
    let active = state.active;
    let laid_out_tab = signal(active.get_untracked());
    let content_state = state.clone();
    let recovery_state = state.clone();
    let recovery_banner_state = state.clone();
    let recovery_open = computed({
        let was_open = Cell::new(false);
        move || {
            // Win32 cannot tween a new editor's first frame. Lay it out before collapsing.
            let open = notices::has_recovery(&recovery_state)
                || (was_open.get()
                    && active.get() != laid_out_tab.get()
                    && !motion::reduced_motion());
            was_open.set(open);
            open
        }
    });
    let snap_offset = signal(0.0);
    let recovery_deadline = Rc::new(Cell::new(None));
    let tab_deadline = Rc::new(Cell::new(None));
    effect({
        let recovery_deadline = recovery_deadline.clone();
        let tab_deadline = tab_deadline.clone();
        move || {
            if motion::reduced_motion()
                && recovery_deadline
                    .take()
                    .max(tab_deadline.take())
                    .is_some_and(|deadline| nsh_core::time::now() < deadline)
            {
                // Win32 ignores frames equal to an active tween's destination.
                // A layout round trip stops the tween without replacing the editor.
                snap_offset.set(1.0);
            }
        }
    });
    view()
        .test_id("column")
        .w_full()
        .max_w(px(layout.measure() + 2.0 * tokens::COLUMN_GAP))
        .h(relative(1.0))
        .flex_col()
        .px(px(tokens::COLUMN_GAP))
        .pt(move || Length::from(px(snap_offset.get())))
        .bg(move || tokens::palette().column_ground)
        .child(when(
            move || !dock_chrome.get(),
            move || {
                let state = recovery_banner_state.clone();
                banner_slot(
                    "recovery-banner-slot",
                    move || recovery_open.get(),
                    move || notices::recovery_banner(state.clone(), language),
                    track_reflow(recovery_deadline.clone()),
                )
            },
        ))
        .child(
            view()
                .test_id("document-slot")
                .flex_1()
                .min_h(px(0.0))
                .ml(px(-tokens::COLUMN_GAP))
                .pl(px(tokens::COLUMN_GAP))
                .flex_col()
                .overflow_hidden()
                .pt(move || Length::from(px(snap_offset.get())))
                // Key on tab identity only: replacing this subtree discards the editor's undo history.
                .child(each(
                    move || vec![active.get()],
                    |key| *key,
                    move |tab, _| {
                        let tab = tab.get_untracked();
                        tab_deadline.set(None);
                        column_content(
                            content_state.clone(),
                            language,
                            dock_chrome,
                            move || {
                                laid_out_tab.set(tab);
                                if snap_offset.get_untracked() != 0.0 {
                                    snap_offset.set(0.0);
                                }
                            },
                            track_reflow(tab_deadline.clone()),
                        )
                    },
                )),
        )
        .child(
            view()
                .w_full()
                .max_w(move || {
                    px(if layout.field_overlays() {
                        (layout.column_width(client_width_now()).min(stage_width())
                            - tokens::COLUMN_GAP)
                            .max(0.0)
                    } else {
                        layout.measure()
                    })
                })
                .child(when(
                    move || !dock_chrome.get(),
                    move || notices::status_line(state.clone(), language, status),
                )),
        )
}

fn track_reflow(deadline: Rc<Cell<Option<nsh_core::time::Instant>>>) -> impl Fn(f32) + Clone {
    move |ms| {
        let end = nsh_core::time::now() + Duration::from_secs_f32(ms / 1000.0);
        deadline.set(deadline.get().max(Some(end)));
    }
}

fn column_content(
    state: AppState,
    language: Language,
    dock_chrome: Memo<bool>,
    on_layout: impl Fn() + 'static,
    on_reflow: impl Fn(f32) + Clone + 'static,
) -> [AnyElement; 2] {
    let document_state = state.clone();
    let conflict_state = state.clone();
    let conflict_banner_state = state.clone();
    [
        when(
            move || !dock_chrome.get(),
            move || {
                let test = conflict_state.clone();
                let draw = conflict_banner_state.clone();
                banner_slot(
                    "conflict-banner-slot",
                    move || notices::has_conflict(&test),
                    move || notices::conflict_banner(draw.clone(), language),
                    on_reflow.clone(),
                )
            },
        )
        .into_element(),
        view()
            .test_id("tab-document-slot")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .on_layout(move |_: &Bounds<Points>| on_layout())
            .child(
                when(move || document::has_document(&document_state), {
                    let state = state.clone();
                    move || document::column(state.clone()).into_element()
                })
                .otherwise(move || welcome::welcome(state.clone(), language).into_element()),
            )
            .into_element(),
    ]
}

/// The narrow Preview stop uses the old full reading column; Split and the
/// other docked surfaces keep the right-hand overlay. Above the breakpoint,
/// the source's trailing edge is the dock's leading edge.
fn dock(
    state: AppState,
    language: Language,
    layout: Layout,
    source_width: Memo<f32>,
    opacity: AnimatedValue,
    status: notices::StatusPresence,
    dock_chrome: Memo<bool>,
) -> impl IntoElement {
    let full_preview = computed({
        let state = state.clone();
        move || shows_full_preview(&state)
    });
    let dock_state = state.clone();
    let narrow = layout.field_overlays();
    let left = move || {
        if !narrow {
            source_width.get()
        } else if full_preview.get() {
            0.0
        } else {
            (layout.field_left(client_width_now()) - tokens::RAIL_W).max(0.0)
        }
    };
    let visible = computed(move || {
        if narrow {
            surface(&dock_state) != FieldSurface::None
        } else {
            left() < stage_width()
                && (surface(&dock_state) != FieldSurface::None
                    || preview_track(&dock_state, layout))
        }
    });
    let column_preview = move || narrow && full_preview.get();
    view()
        .test_id("stage-dock")
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(move || px(left()))
        .w(move || {
            px(if column_preview() {
                stage_width().min(layout.measure() + 2.0 * tokens::COLUMN_GAP)
            } else {
                (stage_width() - left()).max(0.0)
            })
        })
        .pointer_events(move || surface_pointer(visible.get()))
        .overflow_hidden()
        .bind_opacity(&opacity)
        .child(when(
            move || visible.get(),
            move || {
                view()
                    .w_full()
                    .h(relative(1.0))
                    .bg(move || tokens::palette().room)
                    .pl(px(tokens::FIELD_GAP))
                    .pr(move || {
                        px(if column_preview() {
                            tokens::COLUMN_GAP
                        } else {
                            tokens::FIELD_RIGHT_INSET
                        })
                    })
                    .child(
                        view()
                            .w_full()
                            .max_w(move || {
                                px(if full_preview.get() {
                                    layout.measure()
                                } else {
                                    tokens::FIELD_READING_MAX_W
                                })
                            })
                            .h(relative(1.0))
                            .child(field_surface(
                                state.clone(),
                                language,
                                layout,
                                status,
                                dock_chrome,
                            )),
                    )
            },
        ))
}

fn field_surface(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
    dock_chrome: Memo<bool>,
) -> impl IntoElement {
    use crate::state::StatusSurface;
    let notice = state.clone();
    let condition = state.clone();
    let mut field = view()
        .test_id("field")
        .w_full()
        .h(relative(1.0))
        .flex_col()
        .child(when(
            move || {
                matches!(
                    surface(&condition),
                    FieldSurface::Review | FieldSurface::Revision
                )
            },
            move || {
                notices::status_line_at(notice.clone(), language, status, StatusSurface::Review)
            },
        ));
    for want in [
        FieldSurface::Revision,
        FieldSurface::Review,
        FieldSurface::Context,
    ] {
        let condition_state = state.clone();
        let draw_state = state.clone();
        field = field.child(when(
            move || surface(&condition_state) == want,
            move || draw_field(draw_state.clone(), language, want),
        ));
    }
    let preview_state = state.clone();
    field
        .child(when(move || dock_chrome.get(), {
            let state = state.clone();
            move || preview_banners(state.clone(), language)
        }))
        .child(when(
            move || {
                surface(&preview_state) == FieldSurface::Preview
                    || (preview_track(&preview_state, layout)
                        && preview_state.view_track.get() < 1.0)
            },
            {
                let state = state.clone();
                move || preview::field(state.clone(), language)
            },
        ))
        .child(when(
            move || dock_chrome.get(),
            move || notices::status_line(state.clone(), language, status),
        ))
}

fn preview_banners(state: AppState, language: Language) -> impl IntoElement {
    let recovery_open = state.clone();
    let recovery_draw = state.clone();
    let conflict_open = state.clone();
    view()
        .w_full()
        .flex_shrink_0()
        .flex_col()
        .child(banner_slot(
            "recovery-banner-slot",
            move || notices::has_recovery(&recovery_open),
            move || notices::recovery_banner(recovery_draw.clone(), language),
            |_| {},
        ))
        .child(banner_slot(
            "conflict-banner-slot",
            move || notices::has_conflict(&conflict_open),
            move || notices::conflict_banner(state.clone(), language),
            |_| {},
        ))
}

/// Draws the non-preview surface [`surface`] chose.
fn draw_field(state: AppState, language: Language, want: FieldSurface) -> AnyElement {
    match want {
        FieldSurface::Revision => revision::field(state, language).into_element(),
        FieldSurface::Review => review::field(state, language).into_element(),
        FieldSurface::Context => context::field(state, language).into_element(),
        FieldSurface::Preview | FieldSurface::None => unreachable!("not a docked field surface"),
    }
}

/// The overlay layer: the consent card, and the notices guard when it is up.
///
/// The overlay sits above the whole window: the room dims to 30% under it while
/// a consent is waiting, and the card floats — the app's only shadow.
pub(super) fn overlay_layer(state: AppState, language: Language) -> impl IntoElement {
    let consent_state = state.clone();
    let guard_state = state.clone();
    overlay()
        .test_id("overlay")
        .absolute()
        .inset_0()
        // The layer itself never takes a press: empty, it would cover every
        // control in the window. Its scrims and cards still do.
        .pointer_events(PointerEvents::BoxNone)
        .child(modal_presence(
            "consent-layer",
            "consent-motion-card",
            {
                let condition_state = consent_state.clone();
                move || consent::is_open(&condition_state)
            },
            {
                let session = signal(0usize);
                let previous = Cell::new(None);
                effect({
                    let state = consent_state.clone();
                    move || {
                        let kind = consent::kind_of(&state);
                        if previous.replace(kind) != kind && kind.is_some() {
                            session.update(|session| *session += 1);
                        }
                    }
                });
                move || {
                    let state = consent_state.clone();
                    dynamic(move || {
                        session.get();
                        consent::card(state.clone(), language)
                    })
                }
            },
        ))
        .child(modal_presence(
            "guard-layer",
            "guard-motion-card",
            {
                let condition_state = guard_state.clone();
                move || notices::guard_is_open(&condition_state)
            },
            move || notices::guard(guard_state.clone(), language),
        ))
        .child(drop_screen::screen(state, language))
}

/// Mount at the banner's current height; only changes to a mounted slot
/// animate. The child keeps its natural height while the slot clips it.
fn banner_slot<E: IntoElement + 'static>(
    id: &'static str,
    open: impl Fn() -> bool + Clone + 'static,
    content: impl Fn() -> E + 'static,
    on_reflow: impl Fn(f32) + 'static,
) -> impl IntoElement {
    let frame = element_ref();
    let presence = motion::Presence::with_initial(open.clone(), true);
    effect({
        let open = open.clone();
        let before = Cell::new(None);
        move || {
            let open = open();
            if before.replace(Some(open)) == Some(!open) && !motion::reduced_motion() {
                let ms = if open {
                    motion::ENTER_MS
                } else {
                    motion::EXIT_MS
                };
                frame.animate_layout(motion::ease_out_layout(ms));
                on_reflow(ms);
            }
        }
    });
    view()
        .test_id(id)
        .element_ref(&frame)
        .w_full()
        .h({
            let open = open.clone();
            move || if open() { auto() } else { px(0.0).into() }
        })
        .flex_shrink_0()
        .flex_col()
        .overflow_hidden()
        .pointer_events(move || surface_pointer(open()))
        .child(when(
            // Keep comparison state mounted through the fade, including reversals.
            move || presence.visible.get(),
            move || {
                view()
                    .w_full()
                    .flex_col()
                    .flex_shrink_0()
                    .test_id("banner-fade")
                    .bind_opacity(&presence.opacity)
                    .child(content())
            },
        ))
}

fn modal_presence<E: IntoElement + 'static>(
    id: &'static str,
    card_id: &'static str,
    open: impl Fn() -> bool + Clone + 'static,
    content: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let presence = motion::Presence::new(open.clone());
    when(
        move || presence.visible.get(),
        move || {
            view()
                .test_id(id)
                .absolute()
                .inset_0()
                .flex_col()
                .items_center()
                .justify_center()
                .pointer_events({
                    let open = open.clone();
                    move || surface_pointer(open())
                })
                .bind_opacity(&presence.opacity)
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .pointer_events(PointerEvents::None)
                        .bg(move || tokens::palette().scrim()),
                )
                .child(motion::card_geometry(
                    view().test_id(card_id).child(content()),
                    open.clone(),
                ))
        },
    )
}

fn surface_pointer(open: bool) -> PointerEvents {
    if open {
        PointerEvents::Auto
    } else {
        PointerEvents::None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::{Headless, LayoutNode};

    use super::*;
    use crate::state::{GUARD_CANCEL_CHOICE, GUARD_DISCARD_CHOICE, Pane, ReviewUnavailable};

    /// The document the probes open: a heading on its first line, so the
    /// first glyph the column draws is known.
    fn probe_path() -> &'static std::path::Path {
        static FIXTURE: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("plan.md");
                let mut text = String::from("# Release plan\n\nDescribe the next release.\n");
                for index in 4..12 {
                    text.push_str(&format!("line {index}\n"));
                }
                std::fs::write(&path, text).expect("write the fixture");
                (dir, path)
            })
            .1
            .as_path()
    }

    /// A control the test presses: a small box above the shell that runs
    /// `act` on the state.
    fn control(
        id: &'static str,
        state: &AppState,
        act: impl Fn(&AppState) + 'static,
    ) -> impl IntoElement {
        let state = state.clone();
        view()
            .test_id(id)
            .w(px(8.0))
            .h(px(8.0))
            .on_press(move || act(&state))
    }

    /// Sets the active tab's pane.
    fn pane(state: &AppState, pane: Pane) {
        if let Some(tab) = state.active_tab() {
            state.set_pane(tab.id, pane);
        }
    }

    fn recovered_record() -> mt_core::recovery::RecoveredRecord {
        use mt_core::{
            document::io::{FileStamp, Newline, SourceIdentity},
            recovery::{RecoveredRecord, RecoveryKey, RecoveryMetadata, RecoveryRecord},
        };
        RecoveredRecord {
            record: RecoveryRecord {
                key: RecoveryKey::for_document_id("shell-undo-probe"),
                text: "Recovered text".to_owned(),
                metadata: RecoveryMetadata {
                    source_path: None,
                    encoding_name: "UTF-8".to_owned(),
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
                checkpointed_at: std::time::SystemTime::UNIX_EPOCH,
                revision: None,
            },
            source_conflicted: false,
        }
    }

    fn second_recovered_record() -> mt_core::recovery::RecoveredRecord {
        let mut record = recovered_record();
        record.record.key = mt_core::recovery::RecoveryKey::for_document_id("shell-undo-probe-2");
        record
    }

    /// What the app's own last focus move handed the keyboard to, as text a
    /// test can read: one signal per app, so parallel lib tests never share
    /// a focus record.
    fn focus_trace_text(state: &AppState) -> impl IntoElement {
        let state = state.clone();
        text(move || match state.focus_last.get() {
            Some(target) => format!("focus: {target:?}"),
            None => "focus: none".to_owned(),
        })
    }

    /// The shell at `layout` with the fixture open, under controls that open
    /// and close the review and pick the pane. The pane is chosen by the
    /// test, never left to the default.
    fn probe(layout: Layout) -> impl IntoElement {
        let state = AppState::new();
        if let Some(parent) = probe_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(probe_path().to_path_buf());
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(focus_trace_text(&state))
            .child(
                view()
                    .flex_row()
                    .child(control("probe-review-open", &state, |state| {
                        state.review_open.set(true)
                    }))
                    .child(control("probe-review-close", &state, |state| {
                        state.review_open.set(false)
                    }))
                    .child(control("probe-source", &state, |state| {
                        pane(state, Pane::Source)
                    }))
                    .child(control("probe-split", &state, |state| {
                        pane(state, Pane::Split)
                    }))
                    .child(control("probe-preview", &state, |state| {
                        pane(state, Pane::Preview)
                    }))
                    .child(control("probe-dirty", &state, |state| {
                        if let Some(tab) = state.active_tab() {
                            tab.dirty.set(true);
                        }
                    }))
                    .child(control("probe-guard", &state, |state| {
                        if let Some(tab) = state.active_tab() {
                            state.request_close_tab(tab.id);
                        }
                    }))
                    .child(control("probe-forget-focus", &state, |state| {
                        state.focus_last.set(None);
                    }))
                    .child(control("probe-discard-recovered", &state, |state| {
                        state.discard_recovered(&recovered_record());
                    })),
            )
            .child(shell(state, Language::English, layout))
    }

    #[component]
    fn WideProbe() -> impl IntoElement {
        probe(Layout::Wide)
    }

    #[component]
    fn NarrowProbe() -> impl IntoElement {
        probe(Layout::Narrow)
    }

    /// The shell with nothing open: the welcome.
    #[component]
    fn WelcomeProbe() -> impl IntoElement {
        shell(AppState::new(), Language::English, Layout::Wide)
    }

    #[component]
    fn RecoveryBannerProbe() -> impl IntoElement {
        let state = AppState::new();
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-show-recovery", &state, move |state| {
                state.recovery_records.set(vec![recovered_record()]);
            }))
            .child(shell(state, Language::English, Layout::Wide))
    }

    #[component]
    fn ConflictBannerProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let other = state.active.get().expect("untitled tab");
        let dir = tempfile::tempdir().expect("conflict fixture directory");
        let path = dir.path().join("conflict.md");
        std::fs::write(&path, "Original buffer").expect("write fixture");
        state.open_file(path.clone());
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-conflict", &state, move |state| {
                let _keep_file = &dir;
                std::fs::write(&path, "Updated disk contents").expect("external write");
                state.active_tab().expect("file tab").conflict.set(true);
            }))
            .child(control("probe-switch-tab", &state, move |state| {
                let next = if state.active.get() == Some(other) {
                    state.tabs.get().last().expect("file tab").id
                } else {
                    other
                };
                state.set_active(next);
            }))
            .child(control("probe-close-other-tab", &state, move |state| {
                state.request_close_tab(other);
            }))
            .child(control("probe-preview", &state, |state| {
                if let Some(tab) = state.active_tab() {
                    state.set_pane(tab.id, Pane::Preview);
                }
            }))
            .child(control("probe-web", &state, |state| {
                if let Some(tab) = state.active_tab() {
                    state.set_pane(tab.id, Pane::Web);
                }
            }))
            .child(control("probe-rail-switch", &state, move |state| {
                let next = if state.active.get() == Some(other) {
                    state.tabs.get().last().expect("file tab").id
                } else {
                    other
                };
                state.set_active(next);
                state.focus_editor();
            }))
            .child(focus_trace_text(&state))
            .child(shell(state, Language::English, Layout::Wide))
    }

    #[component]
    fn DockedFieldProbe() -> impl IntoElement {
        let state = AppState::new();
        if let Some(parent) = probe_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(probe_path().to_path_buf());
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(control("probe-review", &state, |state| {
                state
                    .review_unavailable
                    .set(Some(ReviewUnavailable::NotSetUp));
            }))
            .child(control("probe-context", &state, |state| {
                state.context_open.set(true);
            }))
            .child(control("probe-revision", &state, |state| {
                present_revision_over_probe(state);
            }))
            .child(shell(state, Language::English, Layout::Medium))
    }

    // The controls sit on one row: a column of them costs the shell height
    // the dock's footers need at 1040 points.
    #[component]
    fn DockAuditProbe() -> impl IntoElement {
        let state = AppState::new();
        if let Some(parent) = probe_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(probe_path().to_path_buf());
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(
                view()
                    .flex_row()
                    .child(control("probe-revision", &state, |state| {
                        present_revision_over_probe(state);
                    }))
                    .child(control("probe-recovery", &state, move |state| {
                        state
                            .recovery_records
                            .set(vec![recovered_record(), second_recovered_record()]);
                    }))
                    .child(control("probe-close-dirty", &state, |state| {
                        if let Some(tab) = state.active_tab() {
                            tab.dirty.set(true);
                            state.request_close_tab(tab.id);
                        }
                    })),
            )
            .child(shell(state, Language::English, Layout::Medium))
    }

    fn present_revision_over_probe(state: &AppState) {
        use mt_core::review::{
            ByteRange, SourceSnapshot,
            revision::{RevisionChange, RevisionEdit, RevisionLimits, RevisionProposal},
        };
        let text = state
            .active_tab()
            .expect("the probe document")
            .text
            .get()
            .to_string();
        let before = "Describe the next release.";
        let start = text.find(before).expect("the probe's paragraph") as u64;
        let range = ByteRange::new(start, start + before.len() as u64).expect("a range");
        let proposal = RevisionProposal::validate(
            &text,
            SourceSnapshot::new(0, 0),
            vec![RevisionChange::new(
                "Name the first release.",
                vec![RevisionEdit::new(
                    range,
                    before,
                    "Describe the first release.",
                )],
            )],
            RevisionLimits::default(),
        )
        .expect("a valid proposal");
        state.present_revision(proposal);
    }

    fn assert_action_whole(app: &Headless, action: &str) {
        assert_action_whole_in(app, "field", action);
    }

    fn assert_action_whole_in(app: &Headless, container: &str, action: &str) {
        let field = node(app, container).bounds;
        let selector = format!("id:{action}");
        let target = app
            .screen()
            .press_target(&selector)
            .expect("a selector")
            .unwrap_or_else(|| panic!("`{action}` is not pressable in `{container}`"));
        let control = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.id == target)
            .expect("the press target is laid out");
        assert!(
            control.bounds.origin.x.0 >= field.origin.x.0
                && control.bounds.origin.x.0 + control.bounds.size.width.0
                    <= field.origin.x.0 + field.size.width.0,
            "`{action}` at {:?} is not whole inside `{container}` {field:?}",
            control.bounds
        );
    }

    fn assert_text_whole_in(app: &Headless, container: &str, label: &str) {
        let field = node(app, container).bounds;
        let nodes = app.screen().layout_nodes();
        let matching: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text.as_str() == label)
            })
            .collect();
        assert!(!matching.is_empty(), "no `{label}` text on screen");
        for control in matching {
            assert!(
                control.bounds.origin.x.0 >= field.origin.x.0
                    && control.bounds.origin.x.0 + control.bounds.size.width.0
                        <= field.origin.x.0 + field.size.width.0,
                "`{label}` at {:?} is not whole inside `{container}` {field:?}",
                control.bounds
            );
        }
    }

    #[test]
    fn the_medium_window_with_the_sidebar_open_keeps_field_actions_whole() {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<DockedFieldProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(1040.0), Points(720.0)))
            .expect("the 1040 point window");
        app.wait_for("text:Describe the next release.")
            .expect("the probe document");
        assert_eq!(
            node(&app, "sidebar").bounds.size.width.0,
            tokens::SIDEBAR_W,
            "the repro needs the sidebar expanded"
        );

        press(&mut app, "probe-review");
        app.wait_for("id:review-unavailable")
            .expect("the review field");
        for action in ["review-open-settings", "review-again", "review-close"] {
            assert_action_whole(&app, action);
        }
        press(&mut app, "review-close");
        app.wait_for_gone("id:review-unavailable", std::time::Duration::from_secs(2))
            .expect("the whole Close action closes the field");

        press(&mut app, "probe-context");
        app.wait_for("id:context-field").expect("the context field");
        for action in ["context-review", "context-refresh", "context-close"] {
            assert_action_whole(&app, action);
        }
        press(&mut app, "context-close");
        app.wait_for_gone("id:context-field", std::time::Duration::from_secs(2))
            .expect("the whole Close action closes the field");

        press(&mut app, "probe-revision");
        app.wait_for("id:revision-field")
            .expect("the revision field");
        for action in ["revision-accept", "revision-reject"] {
            assert_action_whole(&app, action);
        }
    }

    #[component]
    fn ConflictContextProbe() -> impl IntoElement {
        let state = AppState::new();
        let directory = std::rc::Rc::new(tempfile::tempdir().expect("conflict fixture directory"));
        let path = directory.path().join("conflict.md");
        std::fs::write(&path, "Original buffer").expect("write fixture");
        state.open_file(path.clone());
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(control("probe-conflict", &state, move |state| {
                let _keep_directory = &directory;
                std::fs::write(&path, "Updated disk contents").expect("external write");
                if let Some(tab) = state.active_tab() {
                    tab.conflict.set(true);
                }
            }))
            .child(control("probe-context", &state, |state| {
                state.context_open.set(true);
            }))
            .child(shell(state, Language::English, Layout::Medium))
    }

    #[test]
    fn the_conflict_banner_with_context_open_keeps_every_action_whole() {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<ConflictContextProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(1040.0), Points(720.0)))
            .expect("the 1040 point window");
        app.wait_for("text:Original buffer")
            .expect("the conflict fixture loaded");
        press_without_settling(&mut app, "probe-conflict");
        app.wait_for("id:conflict-banner").expect("conflict banner");
        app.settle();
        assert_eq!(
            node(&app, "sidebar").bounds.size.width.0,
            tokens::SIDEBAR_W,
            "the repro needs the sidebar expanded"
        );

        press(&mut app, "probe-context");
        app.wait_for("id:context-field").expect("the context field");
        for action in [
            "conflict-compare",
            "conflict-keep-mine",
            "conflict-use-disk",
            "conflict-save-as",
        ] {
            assert_action_whole_in(&app, "conflict-banner", action);
        }
        press(&mut app, "conflict-compare");
        app.wait_for("id:conflict-previous")
            .expect("the whole Compare action opened the comparison");
    }

    #[component]
    fn ReviewWaitingFooterProbe() -> impl IntoElement {
        use crate::state::{ReviewSection, ReviewView};

        let state = AppState::new();
        state.new_document();
        let owner = state.active.get().expect("the revision's tab");
        state.new_document();
        let shown = state.active.get().expect("the result's tab");
        state.arm_revision_in_flight(owner);
        state.review_view.set(Some(ReviewView {
            sections: vec![ReviewSection {
                title: "Stated goal".to_owned(),
                items: vec!["one".to_owned()],
            }],
            tab_id: Some(shown),
            ..ReviewView::default()
        }));
        state.review_open.set(true);
        view()
            .w_full()
            .h(relative(1.0))
            .child(shell(state, Language::English, Layout::Medium))
    }

    #[test]
    fn the_review_footer_keeps_close_whole_while_a_revision_runs_elsewhere() {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<ReviewWaitingFooterProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(1260.0), Points(720.0)))
            .expect("the 1260 point window");
        app.wait_for("id:review-footer").expect("the review footer");
        app.wait_for("text:Waiting for Revision on another document.")
            .expect("the waiting line widens the footer");
        assert_eq!(
            node(&app, "sidebar").bounds.size.width.0,
            tokens::SIDEBAR_W,
            "the repro needs the sidebar expanded"
        );

        assert_action_whole(&app, "review-close");
        press(&mut app, "review-close");
        app.wait_for_gone("id:review-field", std::time::Duration::from_secs(2))
            .expect("the whole Close action closes the review");
    }

    #[test]
    fn the_dock_and_banners_keep_their_action_rows_whole_at_both_medium_widths() {
        for width in [1040.0, 1260.0] {
            let mut config = EngineConfig::new(
                nsh_testing::windowed_root::<DockAuditProbe>().tokens(tokens::theme()),
            );
            config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
            let mut app = Headless::start(config).expect("start the probe");
            app.set_viewport(nsh_core::Size::new(Points(width), Points(720.0)))
                .expect("a medium window");
            app.wait_for("text:Describe the next release.")
                .expect("the probe document");
            assert_eq!(
                node(&app, "sidebar").bounds.size.width.0,
                tokens::SIDEBAR_W,
                "the repro needs the sidebar expanded at {width}"
            );

            press(&mut app, "probe-revision");
            app.wait_for("id:revision-field")
                .expect("the revision field");
            for action in ["revision-done", "revision-copy", "revision-close"] {
                assert_action_whole(&app, action);
            }

            press(&mut app, "probe-recovery");
            app.wait_for("id:recovery-banner")
                .expect("the recovery banner");
            for action in [
                "recovery-restore-all",
                "recovery-discard-all",
                "recovery-restore-0",
                "recovery-discard-0",
                "recovery-restore-1",
                "recovery-discard-1",
            ] {
                assert_action_whole_in(&app, "recovery-banner", action);
            }

            press(&mut app, "probe-close-dirty");
            app.wait_for("id:guard-card").expect("the guard card");
            for action in ["guard-save", "guard-dont-save", "guard-cancel"] {
                assert_action_whole_in(&app, "guard-card", action);
            }

            assert_text_whole_in(&app, "field", "Undo");
        }
    }

    fn file_probe() -> Headless {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<ConflictBannerProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("the probe's window");
        app.wait_for("text:Original buffer").expect("file loaded");
        app
    }

    fn conflict_probe() -> Headless {
        let mut app = file_probe();
        press_without_settling(&mut app, "probe-conflict");
        app.wait_for("id:conflict-banner").expect("conflict banner");
        app
    }

    #[test]
    fn switching_tabs_does_not_animate_the_conflict_banner() {
        use nsh_core::MountItem;

        let mut app = conflict_probe();
        let batches = capture_batches(&mut app);
        for expected_open in [false, true] {
            press_without_settling(&mut app, "probe-switch-tab");
            let height = node(&app, "conflict-banner-slot").bounds.size.height.0;
            assert_eq!(height > 0.0, expected_open, "banner must follow the tab");
            assert!(
                !batches.try_iter().flatten().any(|item| matches!(
                    item,
                    MountItem::LayoutAnimation(animation) if animation.update.is_some()
                )),
                "switching to a tab with conflict={expected_open} animated layout"
            );
            assert_eq!(
                app.screen()
                    .layout_nodes()
                    .iter()
                    .any(|node| node.test_id.as_deref() == Some("conflict-banner")),
                expected_open,
                "switching must not leave the old banner mounted"
            );
        }
    }

    #[test]
    fn switching_between_equal_conflict_banners_snaps_the_running_reflow() {
        use nsh_core::MountItem;

        let mut app = conflict_probe();
        app.settle();
        let ids = ["conflict-banner-slot", "tab-document-slot"];
        let final_bounds = ids.map(|id| node(&app, id).bounds);
        press_without_settling(&mut app, "probe-switch-tab");
        let batches = capture_batches(&mut app);
        press_without_settling(&mut app, "probe-conflict");
        let entering = ids.map(|id| node(&app, id).id);
        batches
            .try_iter()
            .find(|batch| {
                let frames = transitioned_frames(batch, motion::ENTER_MS);
                entering.iter().all(|id| frames.contains(id))
            })
            .expect("both equal-height target frames must have an entry tween");
        app.run_for(std::time::Duration::from_millis(32))
            .expect("advance partway through the entry tween");
        press_without_settling(&mut app, "probe-switch-tab");
        let switched = ids.map(|id| node(&app, id));
        // Headless reports target frames, not native tween samples.
        assert!(
            switched.each_ref().map(|node| node.bounds) == final_bounds
                && batches.try_iter().any(|batch| {
                    entering.iter().all(|old| {
                        batch.iter().any(|item| matches!(
                            item, MountItem::Delete { id } if id == old
                        ))
                    }) && switched.iter().all(|node| {
                        batch.iter().any(|item| matches!(
                            item, MountItem::Frame { id, .. } if *id == node.id
                        ))
                    }) && !batch.iter().any(|item| matches!(
                        item, MountItem::LayoutAnimation(animation) if animation.update.is_some()
                    ))
                }),
            "the switch must replace the in-flight frames with final frames in one batch"
        );
    }

    #[test]
    fn reduced_motion_during_banner_reflow_preserves_editor_undo() {
        let mut app = file_probe();
        app.type_text("label:Markdown source", "x")
            .expect("edit the file");
        let editor = node(&app, "source-editor").id;
        press_without_settling(&mut app, "probe-conflict");
        app.wait(Duration::from_millis(32)).expect("mid-reflow");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduce motion");
        let same_editor = node(&app, "source-editor").id == editor;
        app.wait_for("id:source-editor >> text:/x/")
            .expect("the typed edit is still present");
        app.type_text("label:Markdown source", "")
            .expect("focus the editor");
        app.press_key("mod+z").expect("undo the edit");
        let undone = app
            .screen()
            .matches("id:source-editor >> text:/^Original buffer$/")
            .expect("read the source text");
        assert!(
            same_editor && undone,
            "editor retained: {same_editor}; undo worked: {undone}"
        );
    }

    #[test]
    fn enabling_reduced_motion_snaps_a_running_banner_reflow() {
        use nsh_core::MountItem;

        let mut app = conflict_probe();
        app.settle();
        let ids = ["conflict-banner-slot", "tab-document-slot"];
        let final_bounds = ids.map(|id| node(&app, id).bounds);
        press_without_settling(&mut app, "probe-switch-tab");
        let batches = capture_batches(&mut app);
        press_without_settling(&mut app, "probe-conflict");
        let moving = ids.map(|id| node(&app, id).id);
        batches
            .try_iter()
            .find(|batch| {
                let frames = transitioned_frames(batch, motion::ENTER_MS);
                moving.iter().all(|id| frames.contains(id))
            })
            .expect("banner and document must have an entry tween");
        app.wait(std::time::Duration::from_millis(32))
            .expect("advance partway through reflow");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduce motion");
        app.settle();
        let snapped = ids.map(|id| node(&app, id));
        assert!(
            snapped.each_ref().map(|node| node.id) == moving
                && snapped.each_ref().map(|node| node.bounds) == final_bounds
                && batches.try_iter().any(|batch| {
                    snapped.iter().all(|node| {
                        batch.iter().any(|item| matches!(
                            item, MountItem::Frame { id, .. } if *id == node.id
                        ))
                    }) && !batch.iter().any(|item| matches!(
                        item, MountItem::LayoutAnimation(animation) if animation.update.is_some()
                    ))
                }),
            "reduced motion must resend the final frames without replacing their nodes"
        );
    }

    #[test]
    fn using_disk_version_keeps_compare_content_mounted_during_banner_exit() {
        fn content(node: &LayoutNode) -> Option<&str> {
            node.text.as_ref().map(|text| text.text.text.as_str())
        }

        let mut app = conflict_probe();
        press_without_settling(&mut app, "conflict-compare");
        app.wait_for("id:conflict-previous").expect("comparison");
        let before = app.screen().layout_nodes();
        let watched: Vec<_> = before
            .iter()
            .filter(|node| {
                node.test_id.as_deref() == Some("conflict-previous")
                    || matches!(
                        content(node),
                        Some("Difference 1 of 1" | "Updated disk contents")
                    )
            })
            .collect();
        assert_eq!(watched.len(), 3, "Compare controls, counter and disk text");
        press_without_settling(&mut app, "conflict-use-disk");
        app.wait_for("id:guard-reload").expect("the reload guard");
        // Answer without Headless's settle step, which would finish the fade.
        press_without_settling(&mut app, "guard-reload");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app
            .screen()
            .matches("id:source-editor >> text:Updated disk contents")
            .unwrap()
        {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            app.next_batch(remaining).expect("reload batch");
        }
        // Headless clips against the final zero-height frame, not the native
        // interpolated frame. Check the mounted nodes rather than visibility.
        let after = app.screen().layout_nodes();
        for expected in watched {
            assert!(
                after
                    .iter()
                    .any(|node| node.id == expected.id && content(node) == content(expected)),
                "Compare content was removed or changed during the fade: {expected:?}"
            );
        }
        app.wait(Duration::from_millis(motion::EXIT_MS as u64 / 2))
            .expect("reach the middle of the conflict exit");
        let comparison = node(&app, "conflict-previous").id;
        let tab_count = |app: &Headless| {
            app.screen()
                .layout_nodes()
                .iter()
                .filter(|node| node.test_id.as_deref() == Some("sidebar-outside-row"))
                .count()
        };
        assert_eq!(tab_count(&app), 2);
        press_without_settling(&mut app, "probe-close-other-tab");
        assert_eq!(tab_count(&app), 1);
        assert_eq!(node(&app, "conflict-previous").id, comparison);
        app.run_for(Duration::from_millis(motion::EXIT_MS as u64 / 2))
            .expect("finish the conflict exit on its original schedule");
        assert!(
            !app.screen()
                .layout_nodes()
                .iter()
                .any(|node| node.id == comparison),
            "closing an inactive tab stranded the fading comparison"
        );
    }

    #[component]
    fn GuardProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let tab = state.active_tab().expect("the untitled tab");
        tab.text.set("Unsaved draft".into());
        tab.dirty.set(true);
        let presses = signal(0);
        let selection = state.guard.clone();
        let document_state = state.clone();
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-close", &state, move |state| {
                presses.update(|count| *count += 1);
                state.request_close_tab(tab.id);
            }))
            .child(control("probe-consent", &state, |state| {
                state.review_consent.set(Some("Consent disclosure".into()));
            }))
            .child(control("probe-consent-cancel", &state, |state| {
                state.cancel_review();
            }))
            .child(text(move || format!("Close presses: {}", presses.get())))
            .child(text(move || {
                match selection.get().map(|guard| guard.choice.get()) {
                    Some(GUARD_CANCEL_CHOICE) => "Guard selection: Cancel",
                    Some(GUARD_DISCARD_CHOICE) => "Guard selection: Don't save",
                    Some(_) => "Guard selection: not Cancel",
                    None => "Guard closed",
                }
            }))
            .child(text(move || {
                document_state
                    .active_tab()
                    .map_or_else(|| "No active document".into(), |tab| tab.text.get())
            }))
            .child(overlay_layer(state, Language::English))
    }

    #[component]
    fn GuardSaveFailureProbe() -> impl IntoElement {
        let directory = tempfile::tempdir().expect("save-failure fixture directory");
        let path = directory.path().join("draft.md");
        std::fs::write(&path, "Saved content").expect("write the save-failure fixture");

        let state = AppState::new();
        state.open_file(path.clone());
        let path_for_failure = path.clone();
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-close", &state, |state| {
                let tab = state.active_tab().expect("the opened document");
                tab.text.set("Edited content".into());
                tab.dirty.set(true);
                state.request_close_tab(tab.id);
            }))
            .child(control("probe-break-save", &state, move |_| {
                let _keep_directory = &directory;
                std::fs::remove_file(&path_for_failure).expect("remove the save destination");
                std::fs::create_dir(&path_for_failure).expect("replace it with a directory");
            }))
            .child(shell(state, Language::English, Layout::Wide))
    }

    #[component]
    fn GuardDiscardProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        state.new_document();
        let tab = state.active_tab().expect("second tab");
        tab.dirty.set(true);
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-close", &state, move |state| {
                state.request_close_tab(tab.id);
            }))
            .child(shell(state, Language::English, Layout::Wide))
    }

    #[test]
    fn discarding_a_guarded_tab_does_not_animate_the_editor_or_rail_removal() {
        use nsh_core::MountItem;
        use std::sync::mpsc;

        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<GuardDiscardProbe>().tokens(tokens::theme()),
        ))
        .expect("start the probe");
        app.wait_for("id:probe-close").expect("close control");
        press(&mut app, "probe-close");
        app.wait_for("id:guard-dont-save").expect("guard open");
        let watched: Vec<_> = app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter_map(|node| match node.test_id.as_deref() {
                Some("source-editor" | "sidebar-outside-row") => {
                    Some((node.id, node.test_id.unwrap()))
                }
                _ => None,
            })
            .collect();
        let (send, receive) = mpsc::channel();
        app.observe(move |_, batch| {
            let mut deletes_animate = false;
            for item in &batch.items {
                match item {
                    MountItem::LayoutAnimation(animation) => {
                        deletes_animate = animation.delete.is_some();
                    }
                    MountItem::Remove { id } | MountItem::Delete { id } => {
                        if let Some((_, name)) = watched.iter().find(|(node, _)| node == id) {
                            send.send((name.clone(), deletes_animate)).unwrap();
                        }
                    }
                    _ => {}
                }
            }
        });
        press(&mut app, "guard-dont-save");
        let removals: Vec<_> = receive.try_iter().collect();
        for expected in ["source-editor", "sidebar-outside-row"] {
            let relevant: Vec<_> = removals
                .iter()
                .filter(|(name, _)| name.as_ref() == expected)
                .collect();
            assert!(
                !relevant.is_empty() && relevant.iter().all(|(_, animated)| !animated),
                "{expected} must be removed without motion: {removals:?}"
            );
        }
    }

    fn open_guard() -> Headless {
        let mut config =
            EngineConfig::new(nsh_testing::windowed_root::<GuardProbe>().tokens(tokens::theme()));
        config.environment.os = nsh_core::platform::Os::Windows;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
        let mut app = Headless::start(config).expect("start the probe");
        app.wait_for("id:probe-close").expect("close control");
        // Headless does not focus an ordinary control on pointer press as Win32 does.
        app.press_key("tab").expect("focus the close control");
        press(&mut app, "probe-close");
        app.wait_for("id:guard-cancel").expect("guard open");
        assert!(app.screen().texts().contains(&"Close presses: 1"));
        app
    }

    #[test]
    fn space_does_not_press_the_focused_control_behind_the_guard() {
        let mut app = open_guard();

        for key in ["space", "shift+space", "shift+enter"] {
            app.press_key(key).expect("press a consumed key");
            let screen = app.screen();
            assert!(screen.matches("id:guard-cancel").expect("selector"));
            assert!(
                screen.texts().contains(&"Close presses: 1"),
                "{key} activated the background close control"
            );
            assert!(
                screen.texts().contains(&"Guard selection: Cancel"),
                "{key} changed the guard selection"
            );
        }

        app.press_key("enter").expect("activate Cancel");
        assert!(app.screen().texts().contains(&"Guard closed"));
        assert!(
            app.screen().texts().contains(&"Close presses: 1"),
            "Enter activated the background close control"
        );
    }

    #[test]
    fn shifted_keys_do_not_discard_the_unsaved_document() {
        let mut app = open_guard();
        app.press_key("shift+tab").expect("select Don't save");
        assert!(app.shows("Guard selection: Don't save"));

        for key in ["shift+space", "shift+enter"] {
            app.press_key(key).expect("press the shifted key");
            assert!(
                app.shows("Guard selection: Don't save"),
                "{key} changed the guard"
            );
            assert!(app.shows("Unsaved draft"), "{key} lost the unsaved text");
            assert!(
                app.shows("Close presses: 1"),
                "{key} pressed the background"
            );
        }

        app.press_key("enter").expect("activate Don't save");
        assert!(app.shows("Guard closed"));
        assert!(app.shows("No active document"));
    }

    #[test]
    fn cancel_after_a_failed_guard_save_keeps_the_notice_and_geometry_during_exit() {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<GuardSaveFailureProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the failed-save probe");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("the probe's window");
        app.wait_for("text:Saved content")
            .expect("the fixture document");

        press_without_settling(&mut app, "probe-close");
        app.wait_for("id:guard-save").expect("the close guard");
        press_without_settling(&mut app, "probe-break-save");
        press_without_settling(&mut app, "guard-save");
        let error = "Could not save this document, so it stays open. Check file permissions or try Save As….";
        app.wait_for(&format!("text:{error}"))
            .expect("the failed-save notice");

        let before_nodes = app.screen().layout_nodes();
        let before_notice = before_nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == error)
            })
            .expect("the failed-save row");
        let before_notice_bounds = before_notice.bounds;
        let before_card_bounds = node(&app, "guard-card").bounds;

        press_without_settling(&mut app, "guard-cancel");

        let after_nodes = app.screen().layout_nodes();
        let notice_row_unchanged = after_nodes.iter().any(|row| {
            row.text
                .as_ref()
                .is_some_and(|text| text.text.text == error)
                && row.bounds == before_notice_bounds
        });
        let card_geometry_unchanged = node(&app, "guard-card").bounds == before_card_bounds;
        assert!(
            notice_row_unchanged && card_geometry_unchanged,
            "Cancel must freeze the failed-save row and card geometry through the exit; row unchanged: {notice_row_unchanged}, card unchanged: {card_geometry_unchanged}"
        );
    }

    fn start(root: Root, width: f32) -> Headless {
        let mut app =
            Headless::start(EngineConfig::new(root.tokens(tokens::theme()))).expect("start");
        app.set_viewport(nsh_core::Size::new(Points(width), Points(900.0)))
            .expect("the probe's window");
        app.wait_for("label:Markdown source")
            .expect("the source editor");
        press(&mut app, "probe-source");
        app
    }

    fn press(app: &mut Headless, id: &str) {
        app.press(&format!("id:{id}"))
            .unwrap_or_else(|why| panic!("press `{id}`: {why}"));
        app.settle();
    }

    /// The first view with test id `id`, as laid out.
    fn node(app: &Headless, id: &str) -> LayoutNode {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no `{id}` on screen: {:?}", app.screen().texts()))
    }

    #[test]
    fn system_theme_switch_updates_the_open_document_without_a_crossfade() {
        use std::sync::{Arc, Mutex};

        use nsh_core::{MountItem, platform::ColorScheme};

        let mut config =
            EngineConfig::new(nsh_testing::windowed_root::<WideProbe>().tokens(tokens::theme()));
        config.environment.color_scheme = ColorScheme::Light;
        config.environment.reduce_motion = false;
        let mut app = Headless::start(config).expect("start");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("the Wide probe's viewport");
        app.wait_for("text:Describe the next release.")
            .expect("document text");
        let shell = node(&app, "shell").id;
        let first_update = Arc::new(Mutex::new(None));
        app.observe({
            let first_update = first_update.clone();
            move |_, batch| {
                for item in &batch.items {
                    if let MountItem::ViewStyle { id, style } = item
                        && *id == shell
                    {
                        first_update
                            .lock()
                            .unwrap()
                            .get_or_insert((style.background_color, style.transition.is_some()));
                    }
                }
            }
        });
        app.set_environment("system appearance becomes dark", |env| {
            env.color_scheme = ColorScheme::Dark;
        })
        .expect("report OS appearance");
        assert_eq!(
            *first_update.lock().unwrap(),
            Some((tokens::DARK.room, false)),
            "the first shell color update must be the dark ground without a native transition"
        );
    }

    fn recovery_probe() -> Headless {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<RecoveryBannerProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("the probe's window");
        app
    }

    fn capture_batches(app: &mut Headless) -> std::sync::mpsc::Receiver<Vec<nsh_core::MountItem>> {
        let (send, receive) = std::sync::mpsc::channel();
        app.observe(move |_, batch| {
            let _ = send.send(batch.items.clone());
        });
        receive
    }

    fn press_without_settling(app: &mut Headless, id: &str) {
        let target = app
            .screen()
            .press_target(&format!("id:{id}"))
            .unwrap()
            .unwrap();
        let engine = app.engine().control();
        engine.dispatch_event(
            target,
            nsh_core::EventKind::Press,
            nsh_core::EventPayload::Press(Default::default()),
        );
        let (send, receive) = std::sync::mpsc::channel();
        engine.flush(move |_| {
            let _ = send.send(());
        });
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        app.catch_up();
    }

    /// A layer close returns the keyboard to the layer still open, else to
    /// the source editor when the pane mounts it, else to the document view;
    /// the pane never changes (contract amendment 5).
    #[test]
    fn a_close_hands_focus_to_the_layer_left_open_or_the_document() {
        let mut app = start(nsh_testing::windowed_root::<WideProbe>(), 1440.0);

        // Preview: the palette closes onto the document view, not the editor.
        press(&mut app, "probe-preview");
        app.wait_for("id:preview-column").expect("the preview");
        app.press_key("mod+p").expect("open the palette");
        app.wait_for("id:palette-input").expect("the palette");
        app.press_key("escape").expect("close the palette");
        app.wait_for_gone("id:palette", std::time::Duration::from_secs(5))
            .expect("the palette closes");
        app.wait_for("text:/focus: Document/")
            .expect("the document view took the keyboard");
        assert!(
            app.screen().matches("id:preview-column").expect("selector"),
            "the pane never switched to Source"
        );

        // The contract's Escape order puts the guard above the palette even
        // though the palette's binding registered last: the first Escape
        // cancels the guard and hands the keyboard to the palette, the
        // second closes the palette.
        press(&mut app, "probe-source");
        press(&mut app, "probe-dirty");
        press(&mut app, "probe-guard");
        app.wait_for("id:guard-card").expect("the guard");
        app.press_key("mod+p").expect("open the palette");
        app.wait_for("id:palette-input").expect("the palette");
        app.press_key("escape").expect("cancel the guard first");
        app.wait_for_gone("id:guard-card", std::time::Duration::from_secs(5))
            .expect("the guard closes");
        app.wait_for("text:focus: Palette")
            .expect("the palette under the guard took the keyboard");
        assert!(
            app.screen().matches("id:palette-input").expect("selector"),
            "the palette under the guard stays open"
        );
        app.press_key("escape").expect("close the palette");
        app.wait_for_gone("id:palette", std::time::Duration::from_secs(5))
            .expect("the palette closes");
        app.wait_for("text:focus: Editor")
            .expect("the editor took the keyboard");

        // Escape closes the title menu and returns the keyboard where the
        // next key lands: in the source editor.
        press(&mut app, "title-settings");
        app.wait_for("id:title-menu").expect("the title menu");
        app.press_key("escape").expect("close the title menu");
        app.wait_for_gone("id:title-menu", std::time::Duration::from_secs(5))
            .expect("the menu closes");
        app.press_key("z").expect("type after the menu closed");
        app.wait_for("id:source-editor >> text:/z/")
            .expect("the keystroke landed in the editor");

        // A press on the scrim outside the menu closes it through the same
        // path as Escape, so the keyboard returns to the editor too.
        press(&mut app, "probe-forget-focus");
        press(&mut app, "title-settings");
        app.wait_for("id:title-menu").expect("the title menu again");
        press(&mut app, "title-menu-scrim");
        app.wait_for_gone("id:title-menu", std::time::Duration::from_secs(5))
            .expect("the outside press closes the menu");
        app.wait_for("text:focus: Editor")
            .expect("the click-away handed the keyboard back to the editor");
        app.press_key("q").expect("type after the outside press");
        app.wait_for("id:source-editor >> text:/q/")
            .expect("the keystroke landed in the editor");

        // The palette over the comparison: its close hands Compare back.
        let mut app = conflict_probe();
        press_without_settling(&mut app, "conflict-compare");
        app.wait_for("id:conflict-previous")
            .expect("the comparison");
        app.press_key("mod+p").expect("open the palette");
        app.wait_for("id:palette-input").expect("the palette");
        app.press_key("escape").expect("close the palette");
        app.wait_for_gone("id:palette", std::time::Duration::from_secs(5))
            .expect("the palette closes");
        app.wait_for("text:focus: Compare")
            .expect("the comparison took the keyboard");
        assert!(
            app.screen()
                .matches("id:conflict-previous")
                .expect("selector"),
            "the comparison below stays open"
        );
    }

    /// A rail switch to a tab whose pane is Preview or Web focuses that
    /// tab's document view: the retry waits for the new subtree instead of
    /// focusing the ref it replaces.
    #[test]
    fn a_tab_switch_focuses_the_new_tabs_document_view() {
        let mut app = file_probe();
        // The file tab (id 2) becomes Preview; the untitled tab (id 1)
        // takes the keyboard; switching back must focus the file tab's
        // document view, not the untitled one it replaces.
        press(&mut app, "probe-preview");
        app.wait_for("id:preview-column").expect("the preview");
        press(&mut app, "probe-rail-switch");
        app.wait_for("text:focus: Editor")
            .expect("the untitled editor took the keyboard");
        press(&mut app, "probe-rail-switch");
        app.wait_for("text:focus: Document(2)")
            .expect("the switched-to tab's document view took the keyboard");
        assert!(
            app.screen().matches("id:preview-column").expect("selector"),
            "the pane stays Preview"
        );

        // The same for a Web pane.
        press(&mut app, "probe-web");
        app.wait_for("id:web-pane").expect("the web pane");
        press(&mut app, "probe-rail-switch");
        app.wait_for("text:focus: Editor")
            .expect("the untitled editor took the keyboard");
        press(&mut app, "probe-rail-switch");
        app.wait_for("text:focus: Document(2)")
            .expect("the web tab's document view took the keyboard");
        assert!(
            app.screen().matches("id:web-pane").expect("selector"),
            "the pane stays Web"
        );
    }

    fn binding_nodes(items: &[nsh_core::MountItem], id: nsh_core::NodeId) -> Vec<u32> {
        items
            .iter()
            .find_map(|item| match item {
                nsh_core::MountItem::AnimationBindings { id: view, bindings } if *view == id => {
                    Some(bindings.iter().map(|(_, node)| *node).collect())
                }
                _ => None,
            })
            .expect("animation bindings")
    }

    #[test]
    fn reopening_a_fading_modal_preserves_the_sampled_opacity_scale_and_rise() {
        use nsh_core::{
            MountItem,
            animation::{AnimationOp, Animator},
        };

        let mut jumps = Vec::new();
        for (open, close, layer, card) in [
            (
                "probe-close",
                "guard-cancel",
                "guard-layer",
                "guard-motion-card",
            ),
            (
                "probe-consent",
                "probe-consent-cancel",
                "consent-layer",
                "consent-motion-card",
            ),
        ] {
            let mut config = EngineConfig::new(
                nsh_testing::windowed_root::<GuardProbe>().tokens(tokens::theme()),
            );
            config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
            let mut app = Headless::start(config).expect("start the probe");
            app.wait_for(&format!("id:{open}")).expect("open control");
            let batches = capture_batches(&mut app);
            press_without_settling(&mut app, open);
            let entry: Vec<_> = batches.try_iter().flatten().collect();
            let layer_id = node(&app, layer).id;
            let card_id = node(&app, card).id;
            let opacity = binding_nodes(&entry, layer_id)[0];
            let [scale, rise] = binding_nodes(&entry, card_id)
                .try_into()
                .expect("two transforms");

            // Headless reports style targets, so sample the renderer's graph.
            let mut animator = Animator::default();
            animator.apply(&MountItem::Animation(AnimationOp::CreateValue {
                node: opacity,
                value: 0.0,
            }));
            for item in &entry {
                animator.apply(item);
            }
            animator.tick(0.0);
            animator.tick(0.04);
            press_without_settling(&mut app, close);
            for item in batches.try_iter().flatten() {
                animator.apply(&item);
            }
            animator.tick(0.08);
            let sample =
                |animator: &Animator| [opacity, scale, rise].map(|node| animator.value(node));
            let exit = sample(&animator);
            assert!(
                (0.05..0.90).contains(&exit[0]),
                "{layer}: expected mid-fade, got {exit:?}"
            );

            press_without_settling(&mut app, open);
            for item in batches.try_iter().flatten() {
                animator.apply(&item);
            }
            animator.tick(0.08);
            let reopen = sample(&animator);
            if layer_id != node(&app, layer).id
                || card_id != node(&app, card).id
                || exit.iter().zip(reopen).any(|(a, b)| (a - b).abs() > 0.001)
            {
                jumps.push(format!("{layer}: {exit:?} to {reopen:?}"));
            }
        }
        assert!(jumps.is_empty(), "modals jumped on reopen: {jumps:?}");
    }

    fn transitioned_frames(items: &[nsh_core::MountItem], ms: f32) -> Vec<nsh_core::NodeId> {
        use nsh_core::MountItem;
        let mut update = None;
        items
            .iter()
            .filter_map(|item| match item {
                MountItem::LayoutAnimation(animation) => {
                    update = animation.update;
                    None
                }
                MountItem::Frame { id, .. } if update == Some(motion::ease_out_layout(ms)) => {
                    Some(*id)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn returning_from_settings_mounts_the_existing_banner_at_its_final_frame() {
        use nsh_core::MountItem;

        let mut app = recovery_probe();
        let batches = capture_batches(&mut app);
        press(&mut app, "probe-show-recovery");
        let ids = ["recovery-banner-slot", "document-slot"];
        let frame_in = |batch: &[MountItem], target| {
            batch.iter().find_map(|item| match item {
                MountItem::Frame { id, frame } if *id == target => Some(*frame),
                _ => None,
            })
        };
        let before: Vec<_> = batches.try_iter().collect();
        let final_frames = ids.map(|id| {
            let target = node(&app, id).id;
            before
                .iter()
                .rev()
                .find_map(|batch| frame_in(batch, target))
        });
        press(&mut app, "title-settings");
        press(&mut app, "title-menu-settings");
        let _ = batches.try_iter().count();
        press_without_settling(&mut app, "settings-close");

        let targets = ids.map(|id| node(&app, id).id);
        let returned: Vec<_> = batches.try_iter().collect();
        let first = returned
            .iter()
            .find(|batch| targets.iter().any(|id| frame_in(batch, *id).is_some()))
            .expect("workspace remount batch");
        let animated = returned.iter().flatten().any(|item| {
            matches!(item, MountItem::LayoutAnimation(animation) if animation.update.is_some())
        });
        assert_eq!(
            (animated, targets.map(|id| frame_in(first, id))),
            (false, final_frames),
            "returning from Settings must mount the banner and document in place without reflow"
        );
    }

    #[test]
    fn revealing_a_banner_grows_its_existing_slot_and_reflows_the_document() {
        let mut app = recovery_probe();
        let before = node(&app, "recovery-banner-slot");
        let document = node(&app, "document-slot");
        let batches = capture_batches(&mut app);
        press(&mut app, "probe-show-recovery");
        let after = node(&app, "recovery-banner-slot");
        let transitioned: Vec<_> = batches
            .try_iter()
            .map(|batch| transitioned_frames(&batch, motion::ENTER_MS))
            .collect();
        assert!(
            before.id == after.id
                && before.bounds.size.height.0 == 0.0
                && after.bounds.size.height.0 > 0.0
                && node(&app, "document-slot").bounds.origin.y.0 > document.bounds.origin.y.0
                && transitioned
                    .iter()
                    .any(|frames| frames.contains(&before.id) && frames.contains(&document.id)),
            "the existing banner slot must expand from zero and move the document in the same transition: {transitioned:?}"
        );
    }

    #[test]
    fn restoring_the_last_draft_keeps_the_banner_mounted_during_its_exit() {
        use nsh_core::{
            MountItem, ViewStyle,
            animation::{AnimationOp, Driver},
            transition::Live,
        };

        let mut app = recovery_probe();
        let no_banner_top = node(&app, "document-slot").bounds.origin.y.0;
        let batches = capture_batches(&mut app);
        press_without_settling(&mut app, "probe-show-recovery");
        app.run_for(Duration::from_millis(motion::ENTER_MS as u64))
            .expect("finish the banner entry");
        let open_slot = node(&app, "recovery-banner-slot");
        let slot = open_slot.id;
        let open_document = node(&app, "document-slot");
        let banner = node(&app, "recovery-banner").id;
        let opacity = binding_nodes(
            &batches.try_iter().flatten().collect::<Vec<_>>(),
            node(&app, "banner-fade").id,
        )[0];
        press_without_settling(&mut app, "recovery-restore-0");
        assert!(app.screen().shows("Recovered text"));

        app.wait(Duration::from_millis(motion::EXIT_MS as u64 / 2))
            .expect("reach the middle of the banner exit");
        assert_eq!(node(&app, "recovery-banner").id, banner);
        let exit: Vec<_> = batches.try_iter().collect();
        let sample = |before: &LayoutNode, after: &LayoutNode| {
            let mut frame = after.bounds;
            if before.id == after.id
                && exit
                    .iter()
                    .any(|batch| transitioned_frames(batch, motion::EXIT_MS).contains(&after.id))
            {
                let mut live = Live::default();
                let mut style = Box::<ViewStyle>::default();
                frame = before.bounds;
                live.move_frame(
                    frame,
                    after.bounds,
                    motion::ease_out_layout(motion::EXIT_MS),
                );
                live.step(0.0, &mut frame, &mut style);
                live.step(f64::from(motion::EXIT_MS) / 2000.0, &mut frame, &mut style);
            }
            frame
        };
        let mid_slot = sample(&open_slot, &node(&app, "recovery-banner-slot"));
        let document = node(&app, "document-slot");
        let mid_document = sample(&open_document, &document);
        assert!(
            mid_slot.size.height.0 > 0.0 && mid_slot.size.height.0 < open_slot.bounds.size.height.0
        );
        let mid_bottom = mid_slot.origin.y.0 + mid_slot.size.height.0;
        assert!(
            (mid_document.origin.y.0 - mid_bottom).abs() < 0.01,
            "the document top {} must follow the collapsing banner bottom {mid_bottom}",
            mid_document.origin.y.0
        );
        let editor = node(&app, "source-editor");
        let mut first_editor = editor.clone();
        first_editor.bounds.size.height = exit
            .iter()
            .flatten()
            .find_map(|item| match item {
                MountItem::Frame { id, frame } if *id == editor.id => Some(frame.size.height),
                _ => None,
            })
            .expect("the new editor's first native frame");
        let mid_editor = sample(&first_editor, &editor);
        let editor_bottom = mid_document.origin.y.0
            + (mid_editor.origin.y.0 - document.bounds.origin.y.0)
            + mid_editor.size.height.0;
        let document_bottom = mid_document.origin.y.0 + mid_document.size.height.0;
        assert!(
            (editor_bottom - document_bottom).abs() < 0.01,
            "the editor bottom {editor_bottom} must fill the current slot ending at {document_bottom}"
        );
        assert!(document.clips, "the document slot must clip its children");
        assert!(editor.bounds.origin.x.0 >= document.bounds.origin.x.0);
        assert!(exit.iter().any(|batch| {
            transitioned_frames(batch, motion::EXIT_MS).contains(&slot)
                && batch.iter().any(|item| {
                    matches!(
                        item,
                        MountItem::Frame { id, frame } if *id == slot && frame.size.height.0 == 0.0
                    )
                })
        }));
        assert!(exit.iter().flatten().any(|item| matches!(
            item,
            MountItem::Animation(AnimationOp::Start {
                node,
                driver: Driver::Timing { to: 0.0, duration_ms, .. },
                ..
            }) if *node == opacity && *duration_ms == motion::EXIT_MS
        )));
        app.run_for(Duration::from_millis(motion::EXIT_MS as u64 / 2))
            .expect("finish the banner exit");
        assert_eq!(node(&app, "document-slot").bounds.origin.y.0, no_banner_top);
        assert!(
            !app.screen()
                .layout_nodes()
                .iter()
                .any(|node| node.test_id.as_deref() == Some("recovery-banner"))
        );
    }

    #[test]
    fn dismissing_a_banner_animates_the_content_reflow_with_its_fade() {
        use nsh_core::{MountItem, animation::AnimationOp};

        let mut app = recovery_probe();
        let batches = capture_batches(&mut app);
        press(&mut app, "probe-show-recovery");
        let slot = node(&app, "recovery-banner-slot");
        let document = node(&app, "document-slot");
        let all: Vec<_> = batches.try_iter().flatten().collect();
        let fade = node(&app, "banner-fade");
        let opacity = binding_nodes(&all, fade.id)[0];
        press(&mut app, "recovery-discard-0");
        app.wait_for_gone("id:recovery-banner", std::time::Duration::from_secs(2))
            .unwrap();
        let exit: Vec<_> = batches.try_iter().collect();
        let transitioned: Vec<_> = exit
            .iter()
            .map(|batch| transitioned_frames(batch, motion::EXIT_MS))
            .collect();
        let mut animator = nsh_core::animation::Animator::default();
        animator.apply(&MountItem::Animation(AnimationOp::CreateValue {
            node: opacity,
            value: 1.0,
        }));
        animator.tick(1.0);
        for item in exit.iter().flatten() {
            if matches!(item, MountItem::Animation(AnimationOp::Start { node, .. }) if *node == opacity)
            {
                animator.apply(item);
            }
        }
        animator.tick(1.0);
        animator.tick(1.05);
        let mid_fade = animator.value(opacity);
        animator.tick(1.2);
        assert!(
            transitioned
                .iter()
                .any(|frames| frames.contains(&slot.id) && frames.contains(&document.id))
                && node(&app, "recovery-banner-slot").bounds.size.height.0 == 0.0
                && mid_fade > 0.0
                && mid_fade < 1.0
                && animator.value(opacity) == 0.0,
            "the slot and document must reflow during the banner's fade: {transitioned:?}, opacity {mid_fade}"
        );
    }

    fn text_x(app: &Headless, text: &str) -> f32 {
        app.screen()
            .layout_nodes()
            .iter()
            .filter_map(|node| node.text.as_ref())
            .filter(|laid| laid.text.text.as_str() == text)
            .map(|laid| laid.content.origin.x.0)
            .reduce(f32::min)
            .unwrap_or_else(|| panic!("no `{text}` on screen: {:?}", app.screen().texts()))
    }

    /// The field's surfaces start one field gap right of the column, once,
    /// for the review and the preview alike.
    #[test]
    fn every_field_surface_starts_one_field_gap_from_the_column() {
        let mut app = start(nsh_testing::windowed_root::<WideProbe>(), 1440.0);
        press(&mut app, "probe-review-open");
        app.wait_for("id:review-lens").expect("the review's lens");
        let review_column = node(&app, "stage-source").bounds;
        let lens = node(&app, "review-lens").bounds.origin.x.0;

        press(&mut app, "probe-review-close");
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview field");
        app.wait_until(
            "the divider reaches Split",
            Duration::from_secs(5),
            |screen| {
                let nodes = screen.layout_nodes();
                let width = |id| {
                    nodes
                        .iter()
                        .find(|node| node.test_id.as_deref() == Some(id))
                        .map_or(0.0, |node| node.bounds.size.width.0)
                };
                width("stage-source") > 0.0 && width("stage-source") == width("stage-dock")
            },
        )
        .expect("the divider reaches the middle");
        let split_column = node(&app, "stage-source").bounds;
        let label = text_x(&app, "Preview");

        let field_x =
            |column: Bounds<Points>| column.origin.x.0 + column.size.width.0 + tokens::FIELD_GAP;
        assert_eq!(
            lens,
            field_x(review_column),
            "the review's lens starts at x{lens}"
        );
        assert_eq!(
            label,
            field_x(split_column),
            "the preview's label starts at x{label}"
        );
    }

    /// Below 1040 the field overlays the right 46% of the window, and its
    /// surface still starts one field gap in.
    #[test]
    fn the_overlay_field_keeps_the_field_gap() {
        let width = 1000.0;
        let mut app = start(nsh_testing::windowed_root::<NarrowProbe>(), width);
        press(&mut app, "probe-review-open");
        app.wait_for("id:review-lens").expect("the review's lens");
        let lens = node(&app, "review-lens").bounds.origin.x.0;
        let expected = width * (1.0 - Layout::OVERLAY_FRACTION) + tokens::FIELD_GAP;
        assert!(
            (lens - expected).abs() < 0.5,
            "the overlay's lens starts at x{lens}, not x{expected}"
        );
    }

    #[test]
    fn a_live_undo_stays_inside_the_visible_editor_column() {
        let mut app = start(Root::of::<NarrowProbe>(), 1000.0);
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview");
        assert_undo_inside_visible_column(&mut app, 1000.0);

        let mut app = start(Root::of::<WideProbe>(), 1280.0);
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview");
        assert_undo_inside_visible_column(&mut app, 1280.0);

        let mut app = start(Root::of::<NarrowProbe>(), 1000.0);
        assert_undo_inside_visible_column(&mut app, 1000.0);
    }

    fn assert_undo_inside_visible_column(app: &mut Headless, width: f32) {
        press(app, "probe-discard-recovered");
        app.wait_for("id:status-undo").expect("the live Undo");
        let column = node(app, "column").bounds;
        let undo = node(app, "status-undo").bounds;
        let visible_right = if Layout::of(width).field_overlays() {
            width * (1.0 - Layout::OVERLAY_FRACTION)
        } else {
            column.origin.x.0 + column.size.width.0
        };

        app.press("id:status-undo")
            .unwrap_or_else(|why| panic!("press the live Undo: {why}"));
        app.wait_for("id:recovery-row-0")
            .expect("the discarded draft returns");

        let left = undo.origin.x.0;
        let right = left + undo.size.width.0;
        assert!(
            left >= column.origin.x.0,
            "the Undo starts at x{left}, left of the column at x{}",
            column.origin.x.0
        );
        assert!(
            right <= visible_right + 0.5,
            "the Undo ends at x{right}, outside the visible column at x{visible_right}"
        );
    }

    #[test]
    fn a_live_undo_stays_clear_of_the_preview_at_220_points() {
        let width = 220.0;
        let mut app = start(Root::of::<NarrowProbe>(), width);
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview");
        press(&mut app, "probe-discard-recovered");
        app.wait_for("id:status-undo").expect("the live Undo");
        assert_undo_left_of_overlay_and_pressable(&mut app, width);
    }

    #[test]
    fn a_live_undo_follows_a_resize_inside_one_split_instance() {
        let mut app = start(Root::of::<NarrowProbe>(), 1000.0);
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview");
        press(&mut app, "probe-discard-recovered");
        app.wait_for("id:status-undo").expect("the live Undo");

        app.set_viewport(nsh_core::Size::new(Points(900.0), Points(900.0)))
            .expect("shrink the window to 900 pt");
        assert_undo_left_of_overlay_and_pressable(&mut app, 900.0);
    }

    fn assert_undo_left_of_overlay_and_pressable(app: &mut Headless, width: f32) {
        let column = node(app, "column").bounds;
        let undo = node(app, "status-undo").bounds;
        let overlay_left = Layout::of(width).field_left(width);
        let left = undo.origin.x.0;
        let right = left + undo.size.width.0;
        assert!(
            left >= column.origin.x.0,
            "the Undo starts at x{left}, left of the column at x{}",
            column.origin.x.0
        );
        assert!(
            right <= overlay_left + 0.5,
            "the Undo is x{left}..{right}, past the overlay's edge at x{overlay_left}"
        );

        app.press("id:status-undo")
            .unwrap_or_else(|why| panic!("press the live Undo: {why}"));
        app.wait_for("id:recovery-row-0")
            .expect("the discarded draft returns");
    }

    /// Opening a review leaves your text where it was: the line numbers it
    /// shows sit in the column's left gap, at one size, the active line's
    /// included.
    #[test]
    fn opening_a_review_does_not_move_the_text() {
        let mut app = start(nsh_testing::windowed_root::<WideProbe>(), 1440.0);
        let first = "# Release plan";
        let before = text_x(&app, first);
        let column = node(&app, "column").bounds;
        let measure_x = column.origin.x.0 + tokens::COLUMN_GAP;
        assert!(
            before >= measure_x && before < measure_x + tokens::SPACE_2,
            "the first glyph at x{before}, not at the measure (x{measure_x})"
        );

        press(&mut app, "probe-review-open");
        app.wait_for("id:review-lens").expect("the review's lens");
        let after = text_x(&app, first);
        assert_eq!(after, before, "opening the review moved the text");

        // The numbers live in the gap between the rail and the measure.
        let numbers: Vec<LayoutNode> = app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|laid| matches!(laid.text.text.as_str(), "1" | "2" | "3"))
            })
            .collect();
        assert_eq!(numbers.len(), 3, "lines 1-3 numbered once each");
        for number in &numbers {
            let left = number.bounds.origin.x.0;
            let right = left + number.bounds.size.width.0;
            assert!(
                left >= column.origin.x.0 && right <= before,
                "a line number spans x{left}..{right}, outside the gap x{}..{before}",
                column.origin.x.0
            );
        }

        // The active line's number (the caret is on line 1) is set like
        // every other: 11px, in the same ink.
        let active = app
            .screen()
            .span_style("id:source-editor >> text:/^1$/")
            .expect("line 1's number");
        let other = app
            .screen()
            .span_style("id:source-editor >> text:/^2$/")
            .expect("line 2's number");
        assert_eq!(active.font_size.0, 11.0, "the active line's number size");
        assert_eq!(other.font_size.0, 11.0, "another line's number size");
        assert_eq!(active.color, other.color, "the active line's number ink");

        press(&mut app, "probe-review-close");
        assert_eq!(
            text_x(&app, first),
            before,
            "closing the review moved the text"
        );
    }

    /// The welcome is not an input, so it draws no caret.
    #[test]
    fn the_welcome_draws_no_caret() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<WelcomeProbe>().tokens(tokens::theme()),
        ))
        .expect("start the probe");
        app.wait_for("id:welcome").expect("the welcome");
        assert!(
            !app.screen().matches("id:welcome-caret").expect("selector"),
            "the welcome draws a caret"
        );
    }

    #[component]
    fn StatusSettingsProbe() -> impl IntoElement {
        let directory = tempfile::tempdir().expect("status fixture directory");
        let path = directory.path().join("status-save.md");
        std::fs::write(&path, "# Saved status\n").expect("write status fixture");
        let state = AppState::new();
        state.open_file(path);
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(control("probe-save", &state, move |state| {
                std::fs::metadata(directory.path()).expect("status fixture remains available");
                state.save_active();
            }))
            .child(control("probe-settings-close", &state, |state| {
                state.settings_open.set(false);
            }))
            .child(shell(state, Language::English, Layout::Wide))
    }

    fn status_settings_app() -> Headless {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<StatusSettingsProbe>().tokens(tokens::theme()),
        );
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        config.environment.reduce_motion = false;
        let mut app = Headless::start(config).expect("start the status-settings probe");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("the status probe's window");
        app.wait_for("label:Markdown source")
            .expect("the saved document");
        app
    }

    #[component]
    fn RoutedStatusProbe() -> impl IntoElement {
        use crate::state::{StatusKind, StatusSurface};
        let state = AppState::new();
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(
                view()
                    .flex_row()
                    .child(control("status-editor", &state, |state| {
                        state.notify_held("Waiting for the editor")
                    }))
                    .child(control("status-review", &state, |state| {
                        state.review_open.set(true);
                        state.notify_at(StatusSurface::Review, StatusKind::Error, "Review failed");
                    }))
                    .child(control("status-settings", &state, |state| {
                        state.settings_open.set(true);
                        state.notify_at(
                            StatusSurface::Settings,
                            StatusKind::Success,
                            "Settings saved",
                        );
                    }))
                    .child(control("status-review-bytes", &state, |state| {
                        state.notify_count_at(
                            StatusSurface::Review,
                            StatusKind::Success,
                            "Sent ",
                            "120 B",
                        );
                    })),
            )
            .child(shell(state, Language::English, Layout::Wide))
    }

    #[component]
    fn ReviewStatusRowProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        state.review_open.set(true);
        state.review_view.set(Some(crate::state::ReviewView {
            sections: vec![crate::state::ReviewSection {
                title: "Intent".to_owned(),
                items: vec!["The document's intent".to_owned()],
            }],
            ..crate::state::ReviewView::default()
        }));
        view()
            .w_full()
            .h(relative(1.0))
            .flex_col()
            .child(
                view()
                    .flex_row()
                    .child(control("review-status-message", &state, |state| {
                        state.notify_at(
                            crate::state::StatusSurface::Review,
                            crate::state::StatusKind::Info,
                            "Review status",
                        );
                    }))
                    .child(control("review-status-clear", &state, |state| {
                        state.status.set(Default::default());
                    }))
                    .child(control("revision-start", &state, |state| {
                        if let Some(tab_id) = state.active.get() {
                            state.arm_revision_in_flight(tab_id);
                        }
                        state.notify_count_at(
                            crate::state::StatusSurface::Review,
                            crate::state::StatusKind::Success,
                            "Sent ",
                            "120 B",
                        );
                    }))
                    .child(control("revision-ready", &state, |state| {
                        state
                            .revision_hunks
                            .set(vec![crate::state::RevisionHunkView {
                                ordinal: 0,
                                change_id: 0,
                                rationale: "Clarify the sentence".to_owned(),
                                before: "Draft".to_owned(),
                                after: "Revised draft".to_owned(),
                                accepted: signal(false),
                            }]);
                        if let Some(tab_id) = state.active.get() {
                            state.finish_revision_in_flight(tab_id);
                        }
                    })),
            )
            .child(shell(state, Language::English, Layout::Wide))
    }

    fn routed_status_app() -> Headless {
        let mut config = EngineConfig::new(nsh_testing::windowed_root::<RoutedStatusProbe>());
        config.environment.color_scheme = nsh_core::platform::ColorScheme::Light;
        let mut app = Headless::start(config).expect("status surfaces");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("wide viewport");
        app
    }

    #[test]
    fn status_stays_with_its_surface() {
        let mut app = routed_status_app();
        for (control, selector, message) in [
            (
                "status-editor",
                "id:column >> text:Waiting for the editor",
                "text:Waiting for the editor",
            ),
            (
                "status-review",
                "id:field >> text:Review failed",
                "text:Review failed",
            ),
            (
                "status-settings",
                "id:settings-page >> text:Settings saved",
                "text:Settings saved",
            ),
        ] {
            app.press(&format!("id:{control}")).expect(control);
            app.wait_for(selector).expect(selector);
            assert_eq!(app.screen().count(message).expect(message), 1);
            assert_eq!(
                app.screen()
                    .span_style(selector)
                    .expect(selector)
                    .font_family
                    .expect(selector)
                    .name()
                    .as_ref(),
                tokens::FONT_UI,
                "{selector}"
            );
        }
    }

    #[test]
    fn status_only_errors_use_danger() {
        let mut app = routed_status_app();
        for (control, selector, expected) in [
            (
                "status-editor",
                "text:Waiting for the editor",
                tokens::LIGHT.ink_rest,
            ),
            ("status-review", "text:Review failed", tokens::LIGHT.danger),
            (
                "status-settings",
                "text:Settings saved",
                tokens::LIGHT.ink_rest,
            ),
        ] {
            app.press(&format!("id:{control}")).expect(control);
            app.wait_for(selector).expect(selector);
            assert_eq!(
                app.screen().span_style(selector).expect(selector).color,
                expected
            );
        }
    }

    #[test]
    fn status_byte_counts_use_the_mono_family() {
        let mut app = routed_status_app();
        app.press("id:status-review")
            .expect("open the review field");
        app.press("id:status-review-bytes")
            .expect("show sent bytes");
        let line = "text:Sent 120 B";
        app.wait_for(line).expect(line);
        assert_eq!(
            app.screen()
                .span_style(line)
                .expect(line)
                .font_family
                .expect("the status message font")
                .name()
                .as_ref(),
            tokens::FONT_UI
        );
        let count = "text:120 B";
        assert_eq!(
            app.screen()
                .span_style(count)
                .expect(count)
                .font_family
                .expect("the status count font")
                .name()
                .as_ref(),
            tokens::FONT_SOURCE
        );
    }

    #[test]
    fn review_status_reserves_its_row_without_a_message() {
        let mut config = EngineConfig::new(
            nsh_testing::windowed_root::<ReviewStatusRowProbe>().tokens(tokens::theme()),
        );
        config.environment.color_scheme = nsh_core::platform::ColorScheme::Light;
        let mut app = Headless::start(config).expect("Review status row");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("wide viewport");
        app.wait_for("id:review-intent")
            .expect("Review body without a message");
        let empty_y = node(&app, "review-intent").bounds.origin.y.0;

        press(&mut app, "review-status-message");
        app.wait_for("text:Review status")
            .expect("Review status message");
        let message_y = node(&app, "review-intent").bounds.origin.y.0;

        press(&mut app, "review-status-clear");
        app.wait_for_gone("text:Review status", std::time::Duration::from_secs(2))
            .expect("cleared Review status message");
        let cleared_y = node(&app, "review-intent").bounds.origin.y.0;
        assert!(
            (empty_y - message_y).abs() <= 0.5 && (empty_y - cleared_y).abs() <= 0.5,
            "Review keeps its status row reserved: empty={empty_y}, message={message_y}, cleared={cleared_y}"
        );
    }

    #[test]
    fn revision_receipt_and_body_position_survive_the_proposal_arrival() {
        let config = EngineConfig::new(
            nsh_testing::windowed_root::<ReviewStatusRowProbe>().tokens(tokens::theme()),
        );
        let mut app = Headless::start(config).expect("Review status row");
        app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
            .expect("wide viewport");
        let receipt = "id:field >> id:status >> text:Sent 120 B";
        app.press("id:revision-start").expect("start the request");
        app.wait_for(receipt).expect("receipt during the request");
        let busy_y = node(&app, "revision-field").bounds.origin.y.0;

        app.press("id:revision-ready")
            .expect("deliver the proposal");
        app.wait_for_gone("id:revision-cancel", Duration::from_secs(2))
            .expect("the request has finished");
        let ready = node(&app, "revision-field").bounds;
        let field = node(&app, "field").bounds;
        let ready_y = ready.origin.y.0;
        let ready_bottom = ready_y + ready.size.height.0;
        let field_bottom = field.origin.y.0 + field.size.height.0;
        assert!(
            app.screen().matches(receipt).expect("receipt selector")
                && (busy_y - ready_y).abs() <= 0.5
                && ready_bottom <= field_bottom + 0.5,
            "the proposal keeps the receipt and fits without moving: busy={busy_y}, ready={ready:?}, field={field:?}"
        );
    }

    fn return_from_settings(
        app: &mut Headless,
        batches: &std::sync::mpsc::Receiver<Vec<nsh_core::MountItem>>,
    ) -> Vec<nsh_core::MountItem> {
        press_without_settling(app, "title-settings");
        app.wait_for("id:title-menu-settings")
            .expect("the Settings menu item");
        press_without_settling(app, "title-menu-settings");
        app.wait_for("id:settings-close")
            .expect("the Settings close control");
        let _ = batches.try_iter().count();
        press_without_settling(app, "probe-settings-close");
        app.catch_up();
        batches.try_iter().flatten().collect()
    }

    fn save_status(app: &mut Headless) {
        press_without_settling(app, "probe-save");
        app.wait_for("text:Saved status-save.md")
            .expect("the save status");
        app.next_frame(std::time::Duration::from_millis(16))
            .expect("start the status timers");
    }

    fn status_animation_starts(items: &[nsh_core::MountItem]) -> (bool, bool) {
        let entry = items.iter().any(|item| {
            matches!(
                item,
                nsh_core::MountItem::Animation(nsh_core::animation::AnimationOp::Start {
                    driver: nsh_core::animation::Driver::Timing { to: 1.0, .. },
                    ..
                })
            )
        });
        let exit = items.iter().any(|item| {
            matches!(
                item,
                nsh_core::MountItem::Animation(nsh_core::animation::AnimationOp::Start {
                    driver: nsh_core::animation::Driver::Timing {
                        to: 0.0,
                        duration_ms,
                        ..
                    },
                    ..
                }) if *duration_ms == motion::EXIT_MS
            )
        });
        (entry, exit)
    }

    #[test]
    fn saving_status_survives_settings_remount_during_hold_and_exit() {
        let mut app = status_settings_app();
        let batches = capture_batches(&mut app);
        let saved = "Saved status-save.md";

        save_status(&mut app);
        app.wait(std::time::Duration::from_millis(
            motion::ENTER_MS as u64 + 32,
        ))
        .expect("finish the status entry");
        assert!(app.screen().shows(saved));

        let visible_return = return_from_settings(&mut app, &batches);
        let (visible_replayed, _) = status_animation_starts(&visible_return);
        let visible_status_returned = app.screen().shows(saved);
        app.wait(std::time::Duration::from_millis(
            motion::STATUS_HOLD_MS + motion::EXIT_MS as u64 + 64,
        ))
        .expect("reach the original status deadline");
        let visible_deadline_preserved = !app.screen().shows(saved);

        save_status(&mut app);
        app.wait(std::time::Duration::from_millis(
            motion::STATUS_HOLD_MS + motion::ENTER_MS as u64 + motion::EXIT_MS as u64 / 2,
        ))
        .expect("reach the status exit");
        let exit_batches: Vec<_> = batches.try_iter().flatten().collect();
        let (_, reached_fade) = status_animation_starts(&exit_batches);
        let fading_status_visible = app.screen().shows(saved);

        let fading_return = return_from_settings(&mut app, &batches);
        let (fading_replayed, _) = status_animation_starts(&fading_return);
        let fading_status_returned = app.screen().shows(saved);
        app.wait(std::time::Duration::from_millis(
            motion::EXIT_MS as u64 + 32,
        ))
        .expect("finish the original status exit");
        let fading_deadline_preserved = !app.screen().shows(saved);

        assert!(
            visible_status_returned
                && !visible_replayed
                && visible_deadline_preserved
                && reached_fade
                && fading_status_visible
                && fading_status_returned
                && !fading_replayed
                && fading_deadline_preserved,
            "visible return={visible_status_returned}, visible replayed={visible_replayed}, visible deadline={visible_deadline_preserved}, reached fade={reached_fade}, fading visible={fading_status_visible}, fading return={fading_status_returned}, fading replayed={fading_replayed}, fading deadline={fading_deadline_preserved}"
        );
    }
}
