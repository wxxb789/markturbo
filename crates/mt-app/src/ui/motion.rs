//! Owner lane: foundation.
//!
//! The Afterglow motion tokens, and [`Heat`]: one emitter's temperature,
//! rising to HOT and cooling to ember on the spec's curve.
//!
//! Frequent changes are instant. Occasional surfaces use short ease-out
//! transitions; the model's light uses a separate thermal curve.
//!
//! Under reduced motion (`reduce_motion()`, the system setting) an ignition is
//! one 100ms crossfade instead of a 90ms flash plus a 1260ms decay.

use std::{cell::Cell, sync::OnceLock};

use nsh_core::{
    Color, LayoutCurve, LayoutTransition,
    element::{
        AnimatedNode, AnimatedValue, Easing, Element, Keyframe, KeyframeTiming, Signal, Spawned,
        Styled, animated_value, effect, reduce_motion, sample_easing, signal, spawn,
    },
};

/// Ignite: how long an emitter takes to reach HOT.
pub const IGNITE_MS: f32 = 90.0;
/// The cooling time constant: `e^(-t/420ms)`.
pub const COOL_TAU_MS: f32 = 420.0;
/// How long the cooling is drawn out: three time constants, where the curve
/// reads as settled (the prototype's 1260ms).
pub const COOL_MS: f32 = 3.0 * COOL_TAU_MS;
/// The afterimage: how long stale emission takes to drain to neutral.
pub const AFTERIMAGE_MS: f32 = 1200.0;
/// Model output that has cooled sits at this level of the heat scale.
pub const EMBER: f32 = 0.18;
/// The filament's charge (hold to send), linear from dull red to white.
pub const FILAMENT_CHARGE_MS: f32 = 900.0;
/// Platform hold delay, longer than the charge so a full hold completes it.
pub const FILAMENT_HOLD_MS: f32 = 1_200.0;
/// Its release: decay toward dull, tau 250ms.
pub const FILAMENT_RELEASE_TAU_MS: f32 = 250.0;
pub const FILAMENT_COOL_MS: f32 = 3.0 * FILAMENT_RELEASE_TAU_MS;
/// A pip lighting, a leaving row's flash.
pub const FLASH_MS: f32 = 120.0;
/// The emitter extinguishes with an ease-in.
pub const EXTINGUISH_MS: f32 = 120.0;
/// Follow moving editor decorations, then only poll for appearance changes.
pub const HEAT_TICK_MS: u64 = 33;
pub const HEAT_WATCH_MS: u64 = 250;
pub const OUTBOUND_DECAY_MS: f32 = 250.0;
/// A local action's hover ink step (never warmth).
pub const HOVER_MS: f32 = 100.0;
const EASE_OUT_POINTS: (f32, f32, f32, f32) = (0.23, 1.0, 0.32, 1.0);
pub const EASE_OUT: Easing = {
    let (x1, y1, x2, y2) = EASE_OUT_POINTS;
    Easing::Bezier(x1, y1, x2, y2)
};
pub const EASE_IN_OUT: Easing = Easing::EaseInOut;
pub const LINEAR: Easing = Easing::Linear;
pub const ENTER_MS: f32 = 200.0;
pub const EXIT_MS: f32 = 150.0;
pub const MENU_ENTER_MS: f32 = 125.0;
pub const MENU_EXIT_MS: f32 = 90.0;
pub const PRESS_RELEASE_MS: f32 = 120.0;
pub const SCALE_FLOOR: f32 = 0.96;
pub const MENU_SCALE: f32 = 0.97;
pub const CARD_RISE_PX: f32 = 4.0;
pub const CARD_RISE_MS: f32 = ENTER_MS;
pub const STATUS_HOLD_MS: u64 = 3000;
pub const CONSENT_NOTICE_MS: u64 = 1500;
/// The rail's names fade as the hover strip widens.
pub const RAIL_WIDEN_MS: f32 = 160.0;
pub const CAMERA_MS: f32 = 360.0;
pub const CAMERA_TICK: std::time::Duration = std::time::Duration::from_micros(16_667);
/// Maximum opacity or heat fade under reduced motion.
pub const REDUCED_CROSSFADE_MS: f32 = 100.0;

pub fn ease_out_layout(ms: f32) -> LayoutTransition {
    let (x1, y1, x2, y2) = EASE_OUT_POINTS;
    LayoutTransition::new(LayoutCurve::bezier(x1, y1, x2, y2), ms)
}

pub fn ease_out<T: Styled>(control: T) -> T {
    let (x1, y1, x2, y2) = EASE_OUT_POINTS;
    control.ease_bezier(x1, y1, x2, y2)
}

/// Keep the drawing and its animation values alive until its fade ends.
/// The caller gates interaction on logical openness, not on visibility.
#[derive(Clone, Copy)]
pub(super) struct Presence {
    pub visible: Signal<bool>,
    pub opacity: AnimatedValue,
}

impl Presence {
    pub fn new(open: impl Fn() -> bool + 'static) -> Self {
        let visible = signal(false);
        let opacity = animated_value(0.0);
        let previous = Cell::new(None);
        let hide = Cell::new(None::<Spawned<()>>);
        effect(move || {
            let open = open();
            let reduced = reduced_motion();
            if previous.replace(Some((open, reduced))) == Some((open, reduced)) {
                return;
            }
            hide.set(None);
            if open {
                visible.set(true);
                let ms = if reduced {
                    REDUCED_CROSSFADE_MS
                } else {
                    ENTER_MS
                };
                opacity.timing(1.0, ms, EASE_OUT);
            } else if reduced {
                opacity.set(0.0);
                visible.set(false);
            } else if visible.get_untracked() {
                let end = opacity.timing(0.0, EXIT_MS, EASE_OUT);
                hide.set(Some(spawn(async move {
                    if end.await {
                        visible.set(false);
                    }
                })));
            }
        });
        Self { visible, opacity }
    }
}

/// Run the card's rise/scale inside its retained owner. An exit freezes the
/// two transforms, so a new entrance can continue from the displayed frame.
pub(super) fn card_geometry<T: Element>(card: T, open: impl Fn() -> bool + 'static) -> T {
    let rise = animated_value(CARD_RISE_PX);
    let scale = animated_value(SCALE_FLOOR);
    effect(move || {
        let open = open();
        if reduced_motion() {
            rise.set(0.0);
            scale.set(1.0);
        } else if open {
            rise.timing(0.0, CARD_RISE_MS, EASE_OUT);
            scale.timing(1.0, ENTER_MS, EASE_OUT);
        } else {
            rise.stop();
            scale.stop();
        }
    });
    if reduced_motion() {
        card
    } else {
        card.bind_scale(&scale).bind_translate_y(&rise)
    }
}

/// The cooling curve `1 - e^(-t/420ms)`, normalised to reach exactly 1 at the
/// last of 16 evenly spaced stops, so it can be played as keyframes: index `i`
/// is the fraction cooled at `i/15` of the animation.
pub const COOL_STOPS: [f32; 16] = [
    0.0000, 0.1908, 0.3470, 0.4748, 0.5795, 0.6652, 0.7354, 0.7929, 0.8399, 0.8784, 0.9100, 0.9358,
    0.9569, 0.9742, 0.9884, 1.0000,
];

/// The camera's curve: 360ms of `cubic-bezier(0.22, 1, 0.36, 1)`.
pub fn camera_easing() -> Easing {
    Easing::Bezier(0.22, 1.0, 0.36, 1.0)
}

/// How far the camera has travelled `elapsed_ms` into its glide: 0 at the
/// start, 1 from [`CAMERA_MS`] on, along [`camera_easing`].
///
/// The column's scroll offset is not a style an [`AnimatedValue`] can drive,
/// so the camera steps it itself; the curve is nsh's own, sampled once a
/// millisecond and read between samples.
pub fn camera_progress(elapsed_ms: f32) -> f32 {
    static CURVE: OnceLock<Vec<f32>> = OnceLock::new();
    let curve = CURVE.get_or_init(|| sample_easing(camera_easing(), CAMERA_MS as usize + 1));
    let at = elapsed_ms.clamp(0.0, CAMERA_MS);
    let index = at.floor() as usize;
    let next = (index + 1).min(curve.len() - 1);
    let fraction = at - index as f32;
    curve[index] + (curve[next] - curve[index]) * fraction
}

/// Whether the system asks for reduced motion. Reading it subscribes.
pub fn reduced_motion() -> bool {
    reduce_motion()
}

/// The heat of an emitter that has just ignited: [`IGNITE_MS`] of rise, then
/// cooling on the sampled curve to [`EMBER`], as one keyframe track.
fn ignition_frames() -> Vec<Keyframe> {
    let rise = IGNITE_MS / (IGNITE_MS + COOL_MS);
    let mut frames = Vec::with_capacity(COOL_STOPS.len() + 2);
    frames.push(Keyframe::at(0.0, 0.0).ease(Easing::Linear));
    for (i, cooled) in COOL_STOPS.iter().enumerate() {
        let u = i as f32 / (COOL_STOPS.len() - 1) as f32;
        let offset = if i == 0 {
            // The top of the rise is where the cooling starts.
            rise
        } else {
            rise + (1.0 - rise) * u
        };
        let heat = 1.0 - (1.0 - EMBER) * cooled;
        frames.push(Keyframe::at(offset, heat).ease(Easing::Linear));
    }
    frames
}

/// The cooling alone, for an emitter that is already lit.
fn cooling_frames() -> Vec<Keyframe> {
    COOL_STOPS
        .iter()
        .enumerate()
        .map(|(i, cooled)| {
            let u = i as f32 / (COOL_STOPS.len() - 1) as f32;
            Keyframe::at(u, 1.0 - (1.0 - EMBER) * cooled).ease(Easing::Linear)
        })
        .collect()
}

/// One emitter's temperature: `0` not emitting, [`EMBER`] model output that
/// has cooled, `1.0` the one HOT item.
///
/// Everything a surface draws for an emitter is an interpolation of this one
/// number — glyph colour, under-bloom, halo, the rail's mark — through
/// [`Heat::glyph`], [`Heat::plate`] and [`Heat::levels`], so an item cannot be
/// hot in one place and cool in another. The bindings read the value
/// reactively: a view that binds them repaints as the heat moves.
///
/// The value lives under the owner that made it (the component) and goes with
/// it.
#[derive(Clone, Copy, Debug)]
pub struct Heat(AnimatedValue);

impl Heat {
    /// An emitter at rest, not emitting: its glyphs are the caller's ink.
    pub fn cold() -> Self {
        Heat(animated_value(0.0))
    }

    /// An emitter holding cooled model output ([`EMBER`]).
    pub fn ember() -> Self {
        Heat(animated_value(EMBER))
    }

    /// The value itself, for a binding no helper covers.
    pub fn value(&self) -> AnimatedValue {
        self.0
    }

    /// Ignite: to HOT in 90ms, then cool to ember on `e^(-t/420ms)`.
    ///
    /// Under reduced motion this is one 100ms crossfade to ember: no flash.
    pub fn ignite(&self) {
        self.0.stop();
        if reduced_motion() {
            self.0
                .timing(EMBER, REDUCED_CROSSFADE_MS, Easing::EaseInOut);
            return;
        }
        // `KeyframeTiming` is `#[non_exhaustive]`: built from its defaults.
        let mut timing = KeyframeTiming::new(IGNITE_MS + COOL_MS);
        timing.easing = Easing::Linear;
        self.0.keyframes(ignition_frames(), timing);
    }

    /// Cool from wherever it is, on the same curve: the item that was hot when
    /// a new one ignites.
    pub fn cool(&self) {
        self.0.stop();
        if reduced_motion() {
            self.0
                .timing(EMBER, REDUCED_CROSSFADE_MS, Easing::EaseInOut);
            return;
        }
        let mut timing = KeyframeTiming::new(COOL_MS);
        timing.easing = Easing::Linear;
        self.0.keyframes(cooling_frames(), timing);
    }

    /// The settled ember level at once: a state shown as if it had already
    /// cooled (a restored review, a screenshot).
    pub fn settle(&self) {
        self.0.stop();
        self.0.set(EMBER);
    }

    /// Extinguish: to nothing in 120ms, ease-in (an accepted proposal, a
    /// dismissed card).
    pub fn extinguish(&self) {
        self.0.stop();
        let ms = if reduced_motion() {
            REDUCED_CROSSFADE_MS
        } else {
            EXTINGUISH_MS
        };
        self.0.timing(0.0, ms, Easing::EaseIn);
    }

    /// The temperature alone, 1 HOT to 0 ember, for a caller that mixes its
    /// own colours.
    pub fn temperature(&self) -> AnimatedNode {
        self.0.interpolate(&[EMBER, 1.0], &[0.0, 1.0])
    }

    /// A glyph colour that follows the heat: `cold` when nothing is emitting,
    /// `ember` when the model's output has cooled, `hot` for the one current
    /// item.
    pub fn glyph(&self, cold: Color, ember: Color, hot: Color) -> AnimatedNode {
        self.0
            .interpolate_colors(&[0.0, EMBER, 1.0], &[cold, ember, hot])
    }

    /// A plate's colour that follows the heat (an under-bloom, a tick, an
    /// added-word ground). The colours carry their own alpha, so a bloom's
    /// opacity moves with its temperature and its geometry never changes.
    pub fn plate(&self, cold: Color, ember: Color, hot: Color) -> AnimatedNode {
        self.glyph(cold, ember, hot)
    }

    /// A number that follows the heat between three levels: a bloom's
    /// thickness, a halo's opacity, a filament's width.
    pub fn levels(&self, cold: f32, ember: f32, hot: f32) -> AnimatedNode {
        self.0.interpolate(&[0.0, EMBER, 1.0], &[cold, ember, hot])
    }
}

impl Default for Heat {
    fn default() -> Self {
        Heat::ember()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_durations_and_beziers_are_tokenized() {
        fn scan(dir: &std::path::Path) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    scan(&path);
                } else if path.file_name().unwrap() != "motion.rs"
                    && path.extension().is_some_and(|ext| ext == "rs")
                {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
                    let numeric_duration = compact.split(".duration(").skip(1).any(|arg| {
                        arg.trim_start_matches(['-', '+', '('])
                            .starts_with(|c: char| c.is_ascii_digit())
                    });
                    assert!(
                        !numeric_duration && !compact.contains("Easing::Bezier("),
                        "move the duration or curve in {} to motion.rs",
                        path.display()
                    );
                }
            }
        }

        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui"));
    }

    #[test]
    fn the_cooling_curve_only_cools() {
        assert_eq!(COOL_STOPS[0], 0.0);
        assert_eq!(COOL_STOPS[15], 1.0);
        for (i, cooled) in COOL_STOPS.iter().enumerate() {
            // Three time constants over the run: stop `i` is `1 - e^(-3i/15)`,
            // normalised to reach 1 at the end — what the prototype's
            // `linear(...)` easing sampled.
            let u = i as f32 / 15.0;
            let expected = (1.0 - (-3.0 * u).exp()) / (1.0 - (-3.0f32).exp());
            assert!((cooled - expected).abs() < 0.001, "stop {i}");
            if i > 0 {
                assert!(*cooled > COOL_STOPS[i - 1], "the curve only ever cools");
            }
        }
    }

    #[test]
    fn the_cool_curve_spans_three_time_constants() {
        assert_eq!(COOL_MS, 1260.0);
        assert_eq!(COOL_TAU_MS, 420.0);
    }

    #[test]
    fn ignition_rises_to_hot_then_cools_to_ember() {
        let frames = ignition_frames();
        let close = |a: f32, b: f32| (a - b).abs() < 0.0001;
        assert!(close(frames[0].value, 0.0), "it starts not emitting");
        assert!(
            close(frames[1].value, 1.0),
            "it reaches HOT at the rise's end"
        );
        assert!(close(
            frames[1].offset.unwrap(),
            IGNITE_MS / (IGNITE_MS + COOL_MS)
        ));
        let last = frames.last().unwrap();
        assert!(close(last.value, EMBER));
        assert_eq!(frames.last().unwrap().offset, Some(1.0));
        // The rise, then nothing but cooling.
        assert!(frames[1].value >= frames[0].value);
        for pair in frames[1..].windows(2) {
            assert!(
                pair[1].value <= pair[0].value,
                "heat never rises again after the ignition"
            );
            assert!(pair[1].offset >= pair[0].offset);
        }
    }

    #[test]
    fn the_camera_glides_out_fast_and_settles_on_its_target() {
        assert_eq!(camera_progress(0.0), 0.0);
        assert_eq!(camera_progress(CAMERA_MS), 1.0);
        assert_eq!(camera_progress(CAMERA_MS * 2.0), 1.0, "it stays put after");
        // `cubic-bezier(0.22, 1, 0.36, 1)` is an ease-out: most of the way is
        // covered in the first half.
        assert!(camera_progress(CAMERA_MS / 2.0) > 0.8);
        let mut last = 0.0;
        for ms in 1..=CAMERA_MS as usize {
            let progress = camera_progress(ms as f32 - 0.5);
            assert!(progress >= last, "the camera never backs up at {ms}ms");
            last = progress;
        }
    }
}
