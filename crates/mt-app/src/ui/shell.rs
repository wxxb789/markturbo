//! Owner lane: foundation.
//!
//! The window shell: one room, four regions, one overlay layer.
//!
//! What the shell owns, and what nothing else may take over:
//!
//! * the opaque title row and the window's own chrome: the system title bar is
//!   hidden and the caption controls' area is reserved (see
//!   [`crate::ui::prims::caption_insets`]), so the room covers the whole
//!   window;
//! * the three layout breakpoints (client width ≥ 1280, 1040–1279, < 1040),
//!   the rail's 44px, the text column's 40 + measure + 40 box and the field's
//!   remaining width with its 40px left and 32px right insets;
//! * below 1040 client width the field becomes an overlay over the right 46%
//!   of the column, and the column dims to 40% beneath it;
//! * the overlay layer the consent card and the notices guard live in;
//! * what the field shows ([`FieldSurface`]) — exactly one surface at a time,
//!   in a fixed order of precedence.
//!
//! Every surface is reached through its module's `is_open` / drawing function;
//! the shell never draws a surface's insides.

use std::{cell::Cell, rc::Rc, time::Duration};

use mt_core::settings::Language;
use nsh_core::{
    Points,
    desktop::{TitleBarOptions, TitleBarStyle, WindowAttributes},
    prelude::*,
};

use crate::{
    state::AppState,
    ui::{
        consent, context, document, motion, notices, preview, rail, review, revision, settings,
        titlebar,
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
    /// The rendered document, in Split.
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

/// The window: title row, rail, column, field and the overlay layer.
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
    hide_system_title_bar();
    let body_state = state.clone();
    view()
        .test_id("shell")
        .flex_1()
        .h(relative(1.0))
        .w_full()
        .flex_col()
        .on_layout(|frame: &Bounds<Points>| {
            // The shell measures itself: `window()` keeps the size the window
            // was opened with until the next resize (Windows), so a harness
            // that sizes the window before the first frame would otherwise get
            // the wrong breakpoint.
            client_width().set(frame.size.width.0);
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

/// The room under the title row: rail, column, field.
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
        .child(rail::rail(state.clone(), language))
        .child(column(state.clone(), language, layout, status))
        .child(field(state, language, layout, status))
}

/// The text column: the document's ground, its measure, its two 40px insets,
/// the banners that belong above the text and the status line at its foot.
///
/// Below 1040 client width it dims to 40% under the field.
fn column(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
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
        .w(px(layout.measure() + 2.0 * tokens::COLUMN_GAP))
        .h(relative(1.0))
        .flex_col()
        .px(px(tokens::COLUMN_GAP))
        .pt(move || Length::from(px(snap_offset.get())))
        .bg(move || tokens::palette().column_ground)
        .when(layout.field_overlays(), |column| {
            column.opacity(Layout::DIMMED)
        })
        .child(banner_slot(
            "recovery-banner-slot",
            move || recovery_open.get(),
            move || notices::recovery_banner(recovery_banner_state.clone(), language),
            track_reflow(recovery_deadline),
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
        .child(notices::status_line(state, language, status))
}

fn track_reflow(deadline: Rc<Cell<Option<nsh_core::time::Instant>>>) -> impl Fn(f32) {
    move |ms| {
        let end = nsh_core::time::now() + Duration::from_secs_f32(ms / 1000.0);
        deadline.set(deadline.get().max(Some(end)));
    }
}

fn column_content(
    state: AppState,
    language: Language,
    on_layout: impl Fn() + 'static,
    on_reflow: impl Fn(f32) + 'static,
) -> [AnyElement; 2] {
    let document_state = state.clone();
    let conflict_state = state.clone();
    let conflict_banner_state = state.clone();
    [
        banner_slot(
            "conflict-banner-slot",
            move || notices::has_conflict(&conflict_state),
            move || notices::conflict_banner(conflict_banner_state.clone(), language),
            on_reflow,
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
                    move || document::column(state.clone(), language).into_element()
                })
                .otherwise(move || welcome::welcome(state.clone(), language).into_element()),
            )
            .into_element(),
    ]
}

/// The field: all remaining width, unbordered, with the 40px field gap as its
/// left inset, the app's 32px right inset and the docked reading position
/// capped at 440px.
///
/// The left inset is the field's, once, for every surface and in both modes
/// (the prototype's `.field > *`): a review's lens line and the preview's
/// label start at the same x, and no surface adds an inset of its own.
///
/// Below 1040 client width it is an opaque overlay across the right 46% of the
/// window, over the dimmed column.
fn field(
    state: AppState,
    language: Language,
    layout: Layout,
    status: notices::StatusPresence,
) -> impl IntoElement {
    let area = view()
        .h(relative(1.0))
        .pl(px(tokens::FIELD_GAP))
        .pr(px(tokens::FIELD_RIGHT_INSET));
    let area = if layout.field_overlays() {
        area.absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .right(px(0.0))
            .w(move || px(client_width_now() * Layout::OVERLAY_FRACTION))
            .bg(move || tokens::palette().room)
    } else {
        area.flex_1()
    };
    area.child(
        view()
            .w_full()
            .max_w(px(tokens::FIELD_READING_MAX_W))
            // The field's full height reaches its surface: a surface that
            // scrolls (the preview) fills it rather than collapsing to 0.
            .h(relative(1.0))
            .child(field_surface(state, language, status)),
    )
}

fn field_surface(
    state: AppState,
    language: Language,
    status: notices::StatusPresence,
) -> impl IntoElement {
    use crate::state::StatusSurface;
    let condition = state.clone();
    let notice = state.clone();
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
            move || notices::status_line_at(notice.clone(), status, StatusSurface::Review),
        ));
    for want in [
        FieldSurface::Revision,
        FieldSurface::Review,
        FieldSurface::Context,
        FieldSurface::Preview,
    ] {
        let condition_state = state.clone();
        let draw_state = state.clone();
        field = field.child(when(
            move || surface(&condition_state) == want,
            move || draw_field(draw_state.clone(), language, want),
        ));
    }
    field
}

/// Draws the surface `[surface]` chose.
fn draw_field(state: AppState, language: Language, want: FieldSurface) -> AnyElement {
    match want {
        FieldSurface::Revision => revision::field(state, language).into_element(),
        FieldSurface::Review => review::field(state, language).into_element(),
        FieldSurface::Context => context::field(state, language).into_element(),
        FieldSurface::Preview => preview::field(state, language).into_element(),
        FieldSurface::None => view().into_element(),
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
                        let kind = consent::is_open(&state)
                            .then(|| state.revision_consent.get().is_some());
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

/// Hides the system title bar: the room runs to the window's top edge, and the
/// native caption controls stay in the area [`prims::caption_insets`] reports.
///
/// It is set once, after the first commit, so the window may show the system
/// bar for a frame while the app starts.
fn hide_system_title_bar() {
    effect(move || {
        // The option structs are `#[non_exhaustive]`: they are built from
        // their defaults and then set field by field.
        let mut title_bar = TitleBarOptions::default();
        title_bar.style = TitleBarStyle::Hidden;
        title_bar.height = Some(Points(tokens::TITLE_ROW_H));
        let mut attributes = WindowAttributes::default();
        attributes.title_bar = Some(title_bar);
        nsh_core::element::desktop::current_window().set(attributes);
    });
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::{Headless, LayoutNode};

    use super::*;
    use crate::state::{GUARD_CANCEL_CHOICE, GUARD_DISCARD_CHOICE, Pane};

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
        use mt_core::{
            document::io::{FileStamp, Newline, SourceIdentity},
            recovery::{RecoveredRecord, RecoveryKey, RecoveryMetadata, RecoveryRecord},
        };

        let state = AppState::new();
        let record = RecoveredRecord {
            record: RecoveryRecord {
                key: RecoveryKey::for_document_id("banner-motion-probe"),
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
        };
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-show-recovery", &state, move |state| {
                state.recovery_records.set(vec![record.clone()]);
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
            .child(shell(state, Language::English, Layout::Wide))
    }

    fn file_probe() -> Headless {
        let mut config =
            EngineConfig::new(Root::of::<ConflictBannerProbe>().tokens(tokens::theme()));
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
        use nsh_core::{EventKind, EventPayload, PayloadValue, ROOT_ID};

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
        // Answer without Headless's settle step, which would finish the fade.
        let dialog = app.pending_dialog().expect("reload confirmation");
        let reload = dialog
            .buttons
            .iter()
            .position(|button| button.text == "Reload")
            .expect("reload action");
        app.engine().control().dispatch_event(
            ROOT_ID,
            EventKind::Dialog,
            EventPayload::Map(vec![
                ("id", PayloadValue::Number(f64::from(dialog.id))),
                ("index", PayloadValue::Number(reload as f64)),
            ]),
        );
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
                .filter(|node| node.test_id.as_deref() == Some("rail-file-row"))
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
            Root::of::<GuardDiscardProbe>().tokens(tokens::theme()),
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
                Some("source-editor" | "rail-file-row") => Some((node.id, node.test_id.unwrap())),
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
        for expected in ["source-editor", "rail-file-row"] {
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
        let mut config = EngineConfig::new(Root::of::<GuardProbe>().tokens(tokens::theme()));
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
        let mut config =
            EngineConfig::new(Root::of::<GuardSaveFailureProbe>().tokens(tokens::theme()));
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
        let error = "Still open: the document could not be saved.";
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

        let mut config = EngineConfig::new(Root::of::<WideProbe>().tokens(tokens::theme()));
        config.environment.color_scheme = ColorScheme::Light;
        config.environment.reduce_motion = false;
        let mut app = Headless::start(config).expect("start");
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
        let mut config =
            EngineConfig::new(Root::of::<RecoveryBannerProbe>().tokens(tokens::theme()));
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
            let mut config = EngineConfig::new(Root::of::<GuardProbe>().tokens(tokens::theme()));
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
        let mut app = start(Root::of::<WideProbe>(), 1440.0);
        let column = node(&app, "column").bounds;
        let field_x = column.origin.x.0 + column.size.width.0 + tokens::FIELD_GAP;

        press(&mut app, "probe-review-open");
        app.wait_for("id:review-lens").expect("the review's lens");
        let lens = node(&app, "review-lens").bounds.origin.x.0;

        press(&mut app, "probe-review-close");
        press(&mut app, "probe-split");
        app.wait_for("id:preview-field").expect("the preview field");
        let label = text_x(&app, "Preview");

        assert_eq!(lens, field_x, "the review's lens starts at x{lens}");
        assert_eq!(label, field_x, "the preview's label starts at x{label}");
    }

    /// Below 1040 the field overlays the right 46% of the window, and its
    /// surface still starts one field gap in.
    #[test]
    fn the_overlay_field_keeps_the_field_gap() {
        let width = 1000.0;
        let mut app = start(Root::of::<NarrowProbe>(), width);
        press(&mut app, "probe-review-open");
        app.wait_for("id:review-lens").expect("the review's lens");
        let lens = node(&app, "review-lens").bounds.origin.x.0;
        let expected = width * (1.0 - Layout::OVERLAY_FRACTION) + tokens::FIELD_GAP;
        assert!(
            (lens - expected).abs() < 0.5,
            "the overlay's lens starts at x{lens}, not x{expected}"
        );
    }

    /// Opening a review leaves your text where it was: the line numbers it
    /// shows sit in the column's left gap, at one size, the active line's
    /// included.
    #[test]
    fn opening_a_review_does_not_move_the_text() {
        let mut app = start(Root::of::<WideProbe>(), 1440.0);
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
            Root::of::<WelcomeProbe>().tokens(tokens::theme()),
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
        let mut config =
            EngineConfig::new(Root::of::<StatusSettingsProbe>().tokens(tokens::theme()));
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
                        state.revision_busy.set(true);
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
                        state.revision_busy.set(false);
                    })),
            )
            .child(shell(state, Language::English, Layout::Wide))
    }

    fn routed_status_app() -> Headless {
        let mut config = EngineConfig::new(Root::of::<RoutedStatusProbe>());
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
        let mut config =
            EngineConfig::new(Root::of::<ReviewStatusRowProbe>().tokens(tokens::theme()));
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
        let config = EngineConfig::new(Root::of::<ReviewStatusRowProbe>().tokens(tokens::theme()));
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
