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

use std::cell::Cell;

use nsh_base::input;
use nsh_core::{
    Color, EventKind, EventPayload,
    element::effect,
    element::{
        Accessible, AdaptiveColor, AnimatedNode, ComponentBuilder, Element, Interactive,
        IntoListener, IntoReactive, ParentElement, SharedString, Signal, StyleRefinement, Styled,
    },
    prelude::*,
};

use crate::ui::{
    motion::{self, HOVER_MS, Heat, PRESS_RELEASE_MS},
    tokens::{
        self, FONT_INPUT, FONT_MODEL, FONT_SOURCE, FONT_UI, HAIRLINE, TYPE_MODEL, TYPE_MODEL_LINE,
        TYPE_UI, TYPE_UI_LINE,
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

/// Caption ink: the quietest text that must still be read (hints,
/// disclosures, secondary lines), at 4.5:1 or better on every ground.
pub fn faint(content: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_caption)
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
            tokens::adaptive_color(|palette| palette.ink),
            tokens::adaptive_color(|palette| palette.ember),
            tokens::adaptive_color(|palette| palette.hot),
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
    press_target(test_id, label.clone())
        .px(px(2.0))
        .py(px(4.0))
        .hover(|s| s.text_color(|| tokens::palette().ink).underline())
        .focus_visible(|s| s.text_color(|| tokens::palette().ink).underline())
        .on_press(on_press)
        .child(
            text(label)
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
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
    press_target(test_id, label.clone())
        .px(px(tokens::SPACE_3))
        .py(px(tokens::SPACE_2))
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().ink_caption)
        .hover(|s| s.border_color(|| tokens::palette().ink))
        .focus_visible(|s| s.border_color(|| tokens::palette().ink))
        .on_press(on_press)
        .child(
            text(label)
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink),
        )
}

/// The control both actions are: a plain view that is the button itself, so
/// its test id — UI Automation's AutomationId — names the element a press
/// reaches and screen readers announce, rather than a view around it.
fn press_target(test_id: &str, label: SharedString) -> View {
    press_feedback(view())
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .optional_label(Some(label))
        // A control is never a drag handle: inside a `window_drag` region (the
        // title row) the innermost view decides, and a press here must reach
        // the control.
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .rounded(px(0.0))
}

/// Pointer press-down snaps the plate; release eases ink over [`PRESS_RELEASE_MS`].
/// Win32 nsh 0.6.1 keyboard activation dispatches `Press`, not `PressDown` or `PressUp`.
pub(crate) fn press_feedback<T: PressControl<Kind>, Kind>(control: T) -> T {
    press_feedback_with_duration(control, press_color_duration())
}

pub(crate) fn press_color_duration() -> Signal<f32> {
    signal(HOVER_MS)
}

pub(crate) fn press_feedback_with_duration<T: PressControl<Kind>, Kind>(
    control: T,
    duration: Signal<f32>,
) -> T {
    let pressed = signal(false);
    let appearance = Cell::new(None::<bool>);
    effect(move || {
        let is_dark = tokens::palette().is_dark;
        if appearance
            .replace(Some(is_dark))
            .is_some_and(|appearance| appearance != is_dark)
        {
            duration.set(0.0);
        }
    });
    motion::ease_out(control.transition_colors().duration(move || duration.get()))
        .feedback_on(EventKind::MouseEnter, move |_| {
            if !pressed.get() {
                duration.set(HOVER_MS);
            }
        })
        .feedback_on(EventKind::MouseLeave, move |_| {
            if !pressed.get() {
                duration.set(HOVER_MS);
            }
        })
        .feedback_on(EventKind::PressDown, move |_| {
            pressed.set(true);
            duration.set(PRESS_RELEASE_MS);
        })
        .feedback_on(EventKind::PressUp, move |_| {
            pressed.set(false);
            duration.set(PRESS_RELEASE_MS);
        })
        .feedback_styles()
}

fn hover_plate(style: StyleRefinement) -> StyleRefinement {
    style.bg(move || tokens::palette().ink_at(0.06))
}

fn pressed_plate(style: StyleRefinement) -> StyleRefinement {
    style
        .transition_none()
        .bg(move || tokens::palette().ink_at(0.16))
}

// nsh gives views and component roots separate builder traits. The adapters
// keep the feedback and its timing identical without replacing native widgets.
pub(crate) enum DirectControl {}
pub(crate) enum ComponentControl {}

pub(crate) trait PressControl<Kind>: Styled {
    fn feedback_styles(self) -> Self;
    fn feedback_on(self, event: EventKind, listener: impl Fn(&EventPayload) + 'static) -> Self;
}

impl<T: Interactive + Styled> PressControl<DirectControl> for T {
    fn feedback_styles(self) -> Self {
        self.hover(hover_plate).active(pressed_plate)
    }

    fn feedback_on(self, event: EventKind, listener: impl Fn(&EventPayload) + 'static) -> Self {
        self.on(event, listener)
    }
}

impl<T: ComponentBuilder + Styled> PressControl<ComponentControl> for T {
    fn feedback_styles(self) -> Self {
        self.hover(hover_plate).active(pressed_plate)
    }

    fn feedback_on(self, event: EventKind, listener: impl Fn(&EventPayload) + 'static) -> Self {
        self.__listen(event, listener.into_listener())
    }
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
                .font_family(FONT_INPUT)
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
        .border_color(move || tokens::palette().ink_caption)
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

/// An animated node interpolating `cold`..`hot` for a caller that mixes its
/// own scale.
pub fn heat_levels(heat: Heat, cold: f32, ember: f32, hot: f32) -> AnimatedNode {
    heat.levels(cold, ember, hot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    /// One of each action, and a line that counts the presses they hear.
    #[component]
    fn Harness() -> impl IntoElement {
        let presses = signal(0u32);
        view()
            .flex_col()
            .child(quiet_action("harness-quiet", "Quiet", move || {
                presses.update(|n| *n += 1)
            }))
            .child(primary_action("harness-primary", "Primary", move || {
                presses.update(|n| *n += 1)
            }))
            .child(
                press_feedback(view())
                    .test_id("harness-row")
                    .on_press(move || presses.update(|n| *n += 1))
                    .child(rest("A row")),
            )
            .child(view().test_id("harness-presses").child(ink(move || {
                SharedString::from(format!("{} presses", presses.get()))
            })))
            .child(faint("A caption"))
    }

    /// A caption is text someone must read: it draws in caption ink, which
    /// reads at 4.5:1 on the room, not in the non-text faint ink.
    #[test]
    fn a_caption_reads_at_4_5_to_1() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<Harness>())).expect("start the harness");
        app.wait_for("text:A caption").expect("the caption");
        let color = app
            .screen()
            .span_style("text:A caption")
            .expect("the caption's style")
            .color;
        let palette = [&tokens::DARK, &tokens::LIGHT]
            .into_iter()
            .find(|p| p.ink_caption == color)
            .unwrap_or_else(|| panic!("the caption is {color:?}, not caption ink"));
        let room = nsh_core::Hsla::from(palette.room);
        let ratio = room.blend(nsh_core::Hsla::from(color)).contrast_ratio(room);
        assert!(ratio >= 4.5, "the caption is {ratio:.2}:1 on the room");
    }

    /// The node carrying `test_id` on the headless screen.
    fn node_with(app: &Headless, test_id: &str) -> nsh_core::NodeId {
        let nodes: Vec<_> = app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter(|node| node.test_id.as_deref() == Some(test_id))
            .collect();
        assert_eq!(nodes.len(), 1, "one view carries `{test_id}`");
        nodes[0].id
    }

    /// An action's test id names the pressable button itself — the element UI
    /// Automation exposes, so its AutomationId — not a plain view around it
    /// that no accessibility tree shows.
    #[test]
    fn an_actions_test_id_is_on_the_button_itself() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<Harness>())).expect("start the harness");
        app.wait_for("id:harness-presses").expect("the harness");
        for (id, name) in [("harness-quiet", "Quiet"), ("harness-primary", "Primary")] {
            let node = node_with(&app, id);
            assert_eq!(
                app.screen()
                    .press_target(&format!("id:{id}"))
                    .expect("selector"),
                Some(node),
                "`{id}` is the control a press goes to"
            );
            assert_eq!(
                app.screen()
                    .press_target(&format!("role:button[{name}]"))
                    .expect("selector"),
                Some(node),
                "`{id}` is the button named {name}"
            );
        }
        app.press("id:harness-quiet")
            .expect("press the quiet action");
        app.press("id:harness-primary")
            .expect("press the primary action");
        app.wait_for("text:2 presses").expect("both presses heard");
    }

    #[test]
    fn all_three_controls_show_the_pressed_plate_before_activation() {
        use nsh_core::geometry::point;

        for id in ["harness-quiet", "harness-primary", "harness-row"] {
            let mut app = Headless::start(EngineConfig::new(Root::of::<Harness>())).expect("start");
            let selector = format!("id:{id}");
            app.wait_for(&selector).expect("control");
            assert_eq!(
                app.screen()
                    .view_style(&selector)
                    .unwrap()
                    .unwrap()
                    .background_color,
                None
            );
            app.drag_and_hold(&selector, point(px(0.0), px(0.0)))
                .expect("pointer down");
            // Headless sends the gesture, but the platform normally reports
            // the native active state separately.
            let node = node_with(&app, id);
            let engine = app.engine().control();
            engine.set_states(node, nsh_core::tailwind::state::PRESSED);
            app.settle();
            let color = app
                .screen()
                .view_style(&selector)
                .unwrap()
                .unwrap()
                .background_color;
            assert!(
                [&tokens::DARK, &tokens::LIGHT]
                    .iter()
                    .any(|palette| color == Some(palette.ink_at(0.16).0)),
                "{id} did not show the pressed plate: {color:?}"
            );
            app.wait_for("text:0 presses")
                .expect("press does not fire on down");
            engine.set_states(node, 0);
            app.drop_held().expect("pointer up");
            app.wait_for("text:1 presses")
                .expect("activated on release");
        }
    }

    #[test]
    fn a_theme_switch_does_not_transition_a_stationary_hover_plate() {
        use std::sync::{Arc, Mutex};

        use nsh_core::{MountItem, platform::ColorScheme};

        let mut config = EngineConfig::new(Root::of::<Harness>().tokens(tokens::theme()));
        config.environment.color_scheme = ColorScheme::Light;
        config.environment.reduce_motion = false;
        let mut app = Headless::start(config).expect("start the harness");
        app.wait_for("id:harness-quiet").expect("the quiet action");
        let control = node_with(&app, "harness-quiet");
        app.engine()
            .control()
            .set_states(control, nsh_core::tailwind::state::HOVERED);
        app.settle();
        assert_eq!(
            app.screen()
                .view_style("id:harness-quiet")
                .unwrap()
                .unwrap()
                .background_color,
            Some(tokens::LIGHT.ink_at(0.06).0),
            "the light hover plate is visible before the theme changes"
        );

        let updates = Arc::new(Mutex::new(Vec::new()));
        app.observe({
            let updates = updates.clone();
            move |_, batch| {
                for item in &batch.items {
                    if let MountItem::ViewStyle { id, style } = item
                        && *id == control
                    {
                        updates
                            .lock()
                            .expect("lock the test record")
                            .push((style.background_color, style.transition));
                    }
                }
            }
        });
        app.set_environment("system appearance becomes dark", |env| {
            env.color_scheme = ColorScheme::Dark;
        })
        .expect("report OS appearance");

        let first_dark_update = updates
            .lock()
            .expect("lock the test record")
            .iter()
            .find(|(background, _)| *background == tokens::DARK.ink_at(0.06))
            .copied()
            .unwrap_or_else(|| panic!("the hovered control never took the dark plate"));
        assert!(
            first_dark_update
                .1
                .is_none_or(|transition| transition.duration == 0.0),
            "the theme's hover plate applies without a transition: {first_dark_update:?}"
        );
    }

    #[test]
    fn press_down_does_not_transition_the_pressed_plate() {
        use std::sync::{Arc, Mutex};

        use nsh_core::MountItem;

        let mut app =
            Headless::start(EngineConfig::new(Root::of::<Harness>())).expect("start the harness");
        app.wait_for("id:harness-presses").expect("the harness");
        app.drag_and_hold(
            "id:harness-quiet",
            nsh_core::geometry::point(px(0.0), px(0.0)),
        )
        .expect("pointer down");
        let node = node_with(&app, "harness-quiet");
        let engine = app.engine().control();
        let transitions = Arc::new(Mutex::new(Vec::new()));
        app.observe({
            let transitions = transitions.clone();
            move |_, batch| {
                for item in &batch.items {
                    if let MountItem::ViewStyle { id, style } = item
                        && *id == node
                    {
                        transitions
                            .lock()
                            .expect("lock the test record")
                            .push(style.transition);
                    }
                }
            }
        });
        engine.set_states(node, nsh_core::tailwind::state::PRESSED);
        app.settle();
        let transition = transitions
            .lock()
            .expect("lock the test record")
            .last()
            .copied()
            .unwrap_or_else(|| panic!("press-down emitted no style update"));
        assert_eq!(transition, None, "the pressed plate snaps on");
        app.wait_for("text:0 presses")
            .expect("press does not fire on down");
        engine.set_states(node, 0);
        app.drop_held().expect("pointer up");
        app.wait_for("text:1 presses")
            .expect("activated on release");
    }
}
