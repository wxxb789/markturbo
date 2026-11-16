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
//!   remaining width with its 32px right inset;
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

/// The field: all remaining width, unbordered, with the app's 32px right inset
/// and the docked reading position capped at 440px.
///
/// Below 1040 client width it is an opaque overlay across the right 46% of the
/// window, over the dimmed column.
fn field(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
    let area = view().h(relative(1.0)).pr(px(tokens::FIELD_RIGHT_INSET));
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
