//! Owner lane: foundation.
//!
//! The window shell: one room, four regions, one overlay layer.
//!
//! ```text
//! ┌ title row 32 ───────────────────────────────────────────────────────┐
//! │ rail 44 │ text column (40 + measure + 40) │ field (rest − 32)      │
//! └──────────────────── status line ──────────────────────────────────┘
//! ```
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

use mt_core::settings::Language;
use nsh_core::{
    Points,
    desktop::{TitleBarOptions, TitleBarStyle, WindowAttributes},
    prelude::*,
};

use crate::{
    state::AppState,
    ui::{
        consent, context, document, notices, preview, rail, review, revision, settings, titlebar,
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
                    move || settings::page(state.clone(), language).into_element()
                },
            )
            .otherwise({
                let state = body_state;
                move || workspace(state.clone(), language, layout).into_element()
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
fn workspace(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
    view()
        .test_id("workspace")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .flex_row()
        .child(rail::rail(state.clone(), language))
        .child(column(state.clone(), language, layout))
        .child(field(state, language, layout))
}

/// The text column: the document's ground, its measure, its two 40px insets,
/// the banners that belong above the text and the status line at its foot.
///
/// Below 1040 client width it dims to 40% under the field.
fn column(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
    let document_state = state.clone();
    let recovery_state = state.clone();
    let conflict_state = state.clone();
    let status_state = state.clone();
    let recovery_banner_state = state.clone();
    let conflict_banner_state = state.clone();
    view()
        .test_id("column")
        .w(px(layout.measure() + 2.0 * tokens::COLUMN_GAP))
        .h(relative(1.0))
        .flex_col()
        .px(px(tokens::COLUMN_GAP))
        .bg(move || tokens::palette().column_ground)
        .when(layout.field_overlays(), |column| {
            column.opacity(Layout::DIMMED)
        })
        .child(when(
            move || notices::has_recovery(&recovery_state),
            move || {
                notices::recovery_banner(recovery_banner_state.clone(), language).into_element()
            },
        ))
        .child(when(
            move || notices::has_conflict(&conflict_state),
            move || {
                notices::conflict_banner(conflict_banner_state.clone(), language).into_element()
            },
        ))
        .child(
            view().flex_1().min_h(px(0.0)).w_full().child(
                when(move || document::has_document(&document_state), {
                    let state = state.clone();
                    move || document::column(state.clone(), language).into_element()
                })
                .otherwise({
                    let state = state.clone();
                    move || welcome::welcome(state.clone(), language).into_element()
                }),
            ),
        )
        .child(notices::status_line(status_state, language))
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
fn field(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
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
            .child(field_surface(state, language)),
    )
}

/// The one surface the field shows, in [`surface`]'s order of precedence.
fn field_surface(state: AppState, language: Language) -> impl IntoElement {
    let mut field = view().test_id("field").w_full().h(relative(1.0)).flex_col();
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
fn overlay_layer(state: AppState, language: Language) -> impl IntoElement {
    let consent_state = state.clone();
    let guard_state = state.clone();
    overlay()
        .test_id("overlay")
        .absolute()
        .inset_0()
        // The layer itself never takes a press: empty, it would cover every
        // control in the window. Its scrims and cards still do.
        .pointer_events(PointerEvents::BoxNone)
        .child(when(
            {
                let condition_state = consent_state.clone();
                move || consent::is_open(&condition_state)
            },
            {
                let state = consent_state.clone();
                move || consent_scrim(state.clone(), language).into_element()
            },
        ))
        .child(when(
            {
                let condition_state = guard_state.clone();
                move || notices::guard_is_open(&condition_state)
            },
            {
                let state = guard_state;
                move || guard_scrim(state.clone(), language).into_element()
            },
        ))
}

/// The dimmed room and the consent card, centred over everything.
fn consent_scrim(state: AppState, language: Language) -> impl IntoElement {
    view()
        .test_id("consent-layer")
        .absolute()
        .inset_0()
        .flex_col()
        .items_center()
        .justify_center()
        .bg(move || tokens::palette().scrim())
        .child(consent::card(state, language))
}

/// The dimmed room and the notices guard, centred over everything.
fn guard_scrim(state: AppState, language: Language) -> impl IntoElement {
    view()
        .absolute()
        .inset_0()
        .flex_col()
        .items_center()
        .justify_center()
        .bg(move || tokens::palette().scrim())
        .child(notices::guard(state, language))
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
    use crate::state::{GUARD_CANCEL_CHOICE, Pane};

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
    fn GuardProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let tab = state.active_tab().expect("the untitled tab");
        tab.dirty.set(true);
        let presses = signal(0);
        let selection = state.guard.clone();
        view()
            .w_full()
            .h(relative(1.0))
            .child(control("probe-close", &state, move |state| {
                presses.update(|count| *count += 1);
                state.request_close_tab(tab.id);
            }))
            .child(text(move || format!("Close presses: {}", presses.get())))
            .child(text(move || {
                match selection.get().map(|guard| guard.choice.get()) {
                    Some(GUARD_CANCEL_CHOICE) => "Guard selection: Cancel",
                    Some(_) => "Guard selection: not Cancel",
                    None => "Guard closed",
                }
            }))
            .child(overlay_layer(state, Language::English))
    }

    #[test]
    fn space_does_not_press_the_focused_control_behind_the_guard() {
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

        for key in ["space", "shift+space"] {
            app.press_key(key)
                .expect("press Space with or without Shift");
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

        app.press_key("shift+enter").expect("press Shift+Enter");
        assert!(app.screen().texts().contains(&"Guard closed"));
        assert!(
            app.screen().texts().contains(&"Close presses: 1"),
            "Shift+Enter activated the background close control"
        );
    }

    /// `root` started in a `width` x 900 window, the editor up, in Source.
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

    /// The left edge of the text `text` draws, wherever it is on screen.
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
}
