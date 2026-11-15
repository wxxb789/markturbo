//! Owner lane: foundation.
//!
//! The small primitives every surface composes: ink text at the spec's
//! levels, the model's voice, quiet and primary actions, a text field, a
//! hairline, emitter plates and `kbd` hints.
//!
//! Two rules live here, because every surface must obey them and neither is
//! easy to check by eye:
//!
//! * Local actions never glow. [`quiet_action`] and [`primary_action`] step
//!   their ink and draw the designed focus ring; no colour in them comes from
//!   the heat scale.
//! * Only model output and outbound data emit. [`model_voice`], [`bloom`],
//!   [`halo`] and [`emitter_plate`] take a [`Heat`] and are the only ways to
//!   draw with the hot/ember colours.

use nsh_base::{button, input};
use nsh_core::{
    Color,
    element::{
        Accessible, AdaptiveColor, AnimatedNode, ComponentBuilder, Easing, Element, Interactive,
        IntoReactive, ParentElement, SharedString, Styled,
    },
    prelude::*,
};

use crate::ui::{
    motion::{HOVER_MS, Heat},
    tokens::{
        self, FONT_MODEL, FONT_SOURCE, FONT_UI, HAIRLINE, TYPE_MODEL, TYPE_MODEL_LINE, TYPE_UI,
        TYPE_UI_LINE,
    },
};

/// The native window controls' area, in points: their height (`top`) and the
/// widths they take at the window's left and right edges. Zero while the
/// system title bar is shown; reading it subscribes.
pub fn caption_insets() -> Edges<Points> {
    nsh_core::element::desktop::title_bar()
}

/// The same colour, fully opaque (a gradient stop, where the plate's opacity
/// carries the heat instead of the colour's alpha).
pub fn opaque(color: Color) -> Color {
    let (r, g, b, _) = color.to_f64();
    Color::rgba(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        0xff,
    )
}

/// Body ink at the UI size and line height: your own text and the labels that
/// are not quiet.
pub fn ink(content: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink)
}

/// Resting ink: quiet local actions and captions.
pub fn rest(content: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_rest)
}

/// Non-text ink: rules, dots, disabled states.
pub fn faint(content: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_faint)
}

/// Source-mode ink: Martian Mono at 14/24, in the given colour.
pub fn source_ink(
    content: impl IntoReactive<SharedString>,
    color: impl IntoReactive<AdaptiveColor>,
) -> impl IntoElement {
    text(content)
        .font_family(FONT_SOURCE)
        .text_size(px(tokens::TYPE_SOURCE))
        .line_height(px(tokens::TYPE_SOURCE_LINE))
        .text_color(color)
}

/// The model's voice: Martian Grotesk 300 at 22/30, the only large type in the
/// app. Its colour follows `heat`, so the one HOT item and everything that has
/// cooled to ember are drawn from one number.
pub fn model_voice(content: impl IntoReactive<SharedString>, heat: Heat) -> impl IntoElement {
    text(content)
        .font_family(FONT_MODEL)
        .font_weight(tokens::WEIGHT_MODEL)
        .text_size(px(TYPE_MODEL))
        .line_height(px(TYPE_MODEL_LINE))
        .text_color(move || tokens::palette().ink)
        .bind_color(&heat.glyph(
            tokens::palette().ink,
            tokens::palette().ember,
            tokens::palette().hot,
        ))
}

/// A quiet local action: resting ink that steps to full ink with an underline
/// on hover, and a designed ring when it has the keyboard focus. It never
/// glows — local actions are lit by ambient light only.
///
/// `test_id` is required: every interactive control carries one.
pub fn quiet_action(
    test_id: &str,
    label: impl Into<SharedString>,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    let label: SharedString = label.into();
    // The test id goes on the wrapper: `nsh_base`'s components are not views,
    // so `test_id` reaches them through a plain element around them.
    view()
        .test_id(test_id)
        .bg(Color::TRANSPARENT)
        // A control is never a drag handle: inside a `window_drag` region (the
        // title row) the innermost view decides, and a press here must reach
        // the button.
        .window_drag(false)
        .child(
            button()
                .accessibility_label(Some(label.clone()))
                .px(px(2.0))
                .py(px(4.0))
                .bg(Color::TRANSPARENT)
                .cursor_pointer()
                .rounded(px(0.0))
                .transition_colors()
                .duration(HOVER_MS)
                .hover(|s| s.text_color(tokens::palette().ink).underline())
                .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                .on_press(on_press)
                .child(
                    text(label)
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink_rest),
                ),
        )
}

/// The state's one next action: full ink, a designed ring, and no heat (it is
/// your action, not the model's).
pub fn primary_action(
    test_id: &str,
    label: impl Into<SharedString>,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    let label: SharedString = label.into();
    view()
        .test_id(test_id)
        .bg(Color::TRANSPARENT)
        .window_drag(false)
        .child(
            button()
                .accessibility_label(Some(label.clone()))
                .px(px(tokens::SPACE_3))
                .py(px(tokens::SPACE_2))
                .bg(Color::TRANSPARENT)
                .cursor_pointer()
                .border(px(HAIRLINE))
                .border_color(move || tokens::palette().ink_faint)
                .transition_colors()
                .duration(HOVER_MS)
                .hover(|s| s.border_color(tokens::palette().ink))
                .focus_visible(|s| s.border_color(tokens::palette().ink))
                .on_press(on_press)
                .child(
                    text(label)
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink),
                ),
        )
}

/// A text field per spec: no plate of its own, a 1px hairline under it, ink
/// text at the UI size, and the field's ground transparent so the room shows
/// through.
pub fn text_field(
    test_id: &str,
    label: &'static str,
    value: impl IntoReactive<String>,
    placeholder: impl IntoReactive<String>,
    on_change: impl Fn(&str) + 'static,
) -> impl ParentElement {
    view()
        .flex_col()
        .child(
            input()
                .test_id(test_id)
                .label(SharedString::from(label))
                .value(value)
                .placeholder(placeholder)
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .bg(Color::TRANSPARENT)
                .on_change(move |text: &str| on_change(text))
                .w_full(),
        )
        .child(hairline())
}

/// The 1px 8% rule that is this direction's only divider.
pub fn hairline() -> impl IntoElement {
    view()
        .w_full()
        .h(px(HAIRLINE))
        .bg(move || tokens::palette().hairline)
}

/// A keyboard hint: `kbd`, Martian Mono 11px with a hairline around it.
pub fn kbd(key: &'static str) -> impl IntoElement {
    view()
        .px(px(tokens::SPACE_1))
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().ink_faint)
        .bg(Color::TRANSPARENT)
        .child(
            text(key)
                .font_family(FONT_SOURCE)
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(move || tokens::palette().ink),
        )
}

/// An emitter's plate. In light theme it is a plate brighter than the room
/// (`#FFFDF7` on `#F2F0EB`); in dark theme there is no plate — the under-bloom
/// is the light — so nothing is drawn.
pub fn emitter_plate(content: impl IntoElement) -> impl IntoElement {
    view()
        .bg(move || {
            let p = tokens::palette();
            if p.is_dark {
                Color::TRANSPARENT
            } else {
                p.emitter_plate
            }
        })
        .child(content)
}

/// The tight under-bloom under an emitter: a statically feathered plate whose
/// opacity follows the heat. Its geometry, its feathering and its blur never
/// change — only how strongly it shows.
///
/// (nsh binds a plate's colour, not a gradient's stops, so the plate is drawn
/// in the hot hue and the temperature reaches the eye through the glyphs and
/// through this plate's opacity.)
pub fn bloom(heat: Heat, height: f32) -> impl IntoElement {
    let alpha = heat.levels(0.0, BLOOM_EMBER, BLOOM_HOT);
    view()
        .h(px(height))
        .w_full()
        .bg_linear_to_b()
        .gradient_from(Color::TRANSPARENT)
        .gradient_via(move || opaque(tokens::palette().hot_bloom))
        .gradient_to(Color::TRANSPARENT)
        .bind_opacity(&alpha)
}

/// The faint halo plate, capped at 6% and 24px: only the single HOT item has
/// one.
pub fn halo(heat: Heat) -> impl IntoElement {
    let alpha = heat.levels(0.0, 0.0, HALO);
    view()
        .absolute()
        .inset(px(-tokens::SPACE_4))
        .bg_radial()
        .gradient_from(move || tokens::palette().halo)
        .gradient_to(Color::TRANSPARENT)
        .bind_opacity(&alpha)
}

/// The under-bloom's alphas at the two ends of the heat scale (the spec's 30%
/// hot, 18% ember).
const BLOOM_HOT: f32 = 0.30;
const BLOOM_EMBER: f32 = 0.18;
/// The halo's cap.
const HALO: f32 = 0.06;

/// The ink step a hover makes, for a caller binding its own transition.
pub fn hover_easing() -> Easing {
    Easing::EaseOut
}

/// An animated node interpolating `cold`..`hot` for a caller that mixes its
/// own scale.
pub fn heat_levels(heat: Heat, cold: f32, ember: f32, hot: f32) -> AnimatedNode {
    heat.levels(cold, ember, hot)
}
