//! Owner lane: review.
//!
//! The document's light: the decorations `document.rs` puts on the source
//! editor so findings move with the text and cannot drift.
//!
//! Contract: [`document_decorations`] is what `document.rs` hands to the
//! editor's `decorations` prop, in the shape `nsh_base::EditorDecoration`
//! takes:
//!
//! * a STATED finding lights the anchored glyphs — an inline range style with
//!   the hot or ember glyph colour, the emitter plate and the tight
//!   under-bloom;
//! * an INFERRED finding lights the air — a `line_style` haze in the
//!   sea-glass phosphor with a 2px gutter edge, while the glyphs stay ink;
//! * every anchored finding gets a 2px heat tick as a gutter mark.
//!
//! The function is called from a reactive binding, so it may read signals; it
//! returns nothing when no review is live, when the review belongs to another
//! tab, or when the document has moved on. It must never mutate state.
//!
//! Emission here is stepped, not engine-animated: decorations carry values,
//! not bindings, so the column's heat is a number this module recomputes on a
//! timer while anything is moving (about 30 steps a second; the spec's stated
//! fallback is a three-step decay). The numbers are the curve the field's
//! [`crate::ui::motion::Heat`] plays — [`IGNITE_MS`] of rise, then
//! `e^(-t/420ms)` to [`EMBER`] — plus [`AFTERIMAGE_MS`] of drain when the
//! source moves on.
//!
//! The review field drives this module: it calls [`observe`] for every result
//! it shows (which resolves the anchors once, against the text the review was
//! made from) and [`focus_finding`] for the finding the user stepped to, so
//! the field's current finding and the column's hot words are one thing.

use std::time::Duration;

use mt_core::review::SourceLocation;
use nsh_base::editor::{
    EditorDecoration, EditorGutterMark, EditorGutterPlacement, EditorPosition, EditorRange,
};
use nsh_core::{
    Color, PropertyValue, StyleDecl, StyleKey,
    element::Style,
    prelude::*,
    time::{Instant, now},
};

use crate::{
    state::{AppState, ReviewFindingView, ReviewView, Tab},
    ui::{
        motion::{
            AFTERIMAGE_MS, COOL_MS, COOL_TAU_MS, EMBER, IGNITE_MS, REDUCED_CROSSFADE_MS,
            STAGGER_MS, reduced_motion,
        },
        tokens::{self, Palette, with_alpha},
    },
};

/// How long the stepped clock waits between steps while the column is
/// moving: about 30 a second, near the display's rate without repainting
/// decorations every frame.
const TICK_MS: u64 = 33;

/// How long the clock waits between beats once the column has settled: a
/// decoration's style is a snapshot of the palette, and nsh-base's
/// decorations effect does not re-run when the appearance changes, so the
/// clock samples the palette while a result is live and repaints only when
/// it differs.
const WATCH_MS: u64 = 250;

/// One review's emission in the column: which result it belongs to, when the
/// arrival sequence began, which finding is HOT, and the anchors resolved
/// once against the text the review was made from.
///
/// One per window (a [`global`]), so it survives the shell being rebuilt
/// across a language or layout change. The decorations read it; [`observe`]
/// and [`focus_finding`] write it.
#[derive(Clone, Default)]
struct Emission {
    /// The result the schedule belongs to; `None` when nothing is live.
    view: Option<ReviewView>,
    /// The tab the anchors were resolved in; another tab's text never
    /// carries them.
    tab_id: Option<u64>,
    /// One slot per finding, in the field's order; `None` for a finding
    /// with no anchor in this document (document-wide, another file).
    anchors: Vec<Option<Anchor>>,
    /// When the arrival sequence began.
    arrived: Option<Instant>,
    /// The finding whose words are HOT, and when it took the focus, in ms
    /// from the arrival.
    focus: Option<usize>,
    focus_ms: f32,
    /// When each finding began cooling after losing the focus, in ms from
    /// the arrival.
    cooling_since: Vec<Option<f32>>,
    /// When the emission went stale, in ms from the arrival; the afterimage
    /// drains from then.
    stale_ms: Option<f32>,
    /// Everything has settled by this time (ms from the arrival).
    motion_until_ms: f32,
    /// The appearance the styles were built under
    /// ([`palette`](tokens::palette)`().is_dark`); the clock repaints when
    /// the window's differs.
    appearance: bool,
    /// Flips on every repaint. A decoration's *style* is not what the
    /// editor's rows watch — a row rebuilds its spans only when its content
    /// changes — so while a result is live every anchored finding also
    /// carries a styleless twin decoration whose presence follows this flag:
    /// the row's content changes, and it reads the styles the clock just
    /// wrote.
    pulse: bool,
    /// The stepped clock: what a repaint reads.
    now_ms: f32,
}

/// One finding's anchor, in the editor's own coordinates.
#[derive(Clone)]
struct Anchor {
    /// The range the light sits on (the anchored words of a stated finding,
    /// the rows of an inferred one).
    range: EditorRange,
    /// The anchored glyphs themselves emit; else the air beside them does.
    stated: bool,
    /// When this finding ignites, in ms from the arrival (reading order).
    at_ms: f32,
}

/// The emission of this window.
fn emission() -> Signal<Emission> {
    global::<Emission>()
}

/// Whether the stepped clock is running, and its pending step's handle.
fn ticker_running() -> Signal<bool> {
    global::<bool>()
}

fn ticker_step() -> Signal<Option<Spawned<()>>> {
    global::<Option<Spawned<()>>>()
}

/// Registers the result the field is showing: a new one starts the arrival
/// sequence and resolves its anchors against `tab`'s text; a result that
/// just went stale begins the afterimage drain; `None` ends everything.
///
/// Called from an effect, never from a render: it writes the emission the
/// decorations read.
pub fn observe(view: Option<&ReviewView>, tab: Option<&Tab>) {
    let signal = emission();
    let current = signal.get_untracked();
    let reduced = reduced_motion();
    let Some(view) = view else {
        if current.view.is_some() {
            signal.set(Emission::default());
        }
        return;
    };
    if current
        .view
        .as_ref()
        .is_some_and(|shown| shown.same_result(view))
    {
        if view.stale && current.stale_ms.is_none() {
            let at = elapsed_ms(&current);
            signal.update(|e| {
                e.stale_ms = Some(at);
                e.motion_until_ms = e.motion_until_ms.max(at + afterimage_ms(reduced));
            });
            ensure_ticker();
        }
        return;
    }
    // A new result: resolve its anchors and start the arrival in reading
    // order. The anchors are resolved here, once, against the text the
    // review was made from; a repaint never re-resolves them, and the editor
    // carries them through edits.
    let Some(tab) = tab else {
        signal.set(Emission::default());
        return;
    };
    let text = tab.text.get_untracked();
    let anchors = resolve_anchors(view, &text, reduced);
    let motion_until = anchors
        .iter()
        .flatten()
        .map(|anchor| anchor.at_ms)
        .fold(0.0_f32, f32::max);
    let motion_until = if anchors.is_empty() {
        0.0
    } else {
        motion_until + ignition_ms(reduced) + cooling_ms(reduced)
    };
    // A result that arrives already stale is drained from the start.
    let stale_ms = view.stale.then_some(-afterimage_ms(reduced));
    signal.set(Emission {
        view: Some(view.clone()),
        tab_id: Some(tab.id),
        anchors,
        arrived: Some(now()),
        focus: None,
        focus_ms: 0.0,
        cooling_since: vec![None; view.findings_anchored.len()],
        stale_ms,
        motion_until_ms: motion_until,
        appearance: tokens::palette().is_dark,
        pulse: false,
        now_ms: 0.0,
    });
    ensure_ticker();
}

/// Focuses one finding of the live result: its words go HOT, the one before
/// it cools, and the column repaints on the stepped clock.
pub fn focus_finding(finding: Option<usize>) {
    let signal = emission();
    let reduced = reduced_motion();
    let at = {
        let e = signal.get_untracked();
        if e.view.is_none() {
            return;
        }
        elapsed_ms(&e)
    };
    let mut heating = false;
    signal.update(|e| {
        if e.focus == finding {
            return;
        }
        if let Some(previous) = e.focus
            && let Some(since) = e.cooling_since.get_mut(previous)
        {
            *since = Some(at);
        }
        e.focus = finding;
        e.focus_ms = at;
        if finding.is_some() {
            heating = true;
            e.motion_until_ms = e
                .motion_until_ms
                .max(at + ignition_ms(reduced) + cooling_ms(reduced));
        }
    });
    if heating {
        ensure_ticker();
    }
}

/// The 1-based, inclusive line range `finding` is anchored to, against the
/// text it was reviewed from: what the field writes as `lines 14–16`.
/// `None` for a document-wide finding or one anchored in another file.
pub fn finding_lines(finding: &ReviewFindingView, text: &str) -> Option<(u64, u64)> {
    let location = finding.location?;
    let (start, end) = anchored_bytes(text, location)?;
    let first = line_of(text, start) + 1;
    let last = line_of(text, end.saturating_sub(1)) + 1;
    Some((first, last))
}

/// The live decorations for `tab`'s source editor: stated glyphs, inferred
/// haze and one gutter tick per anchored finding. Empty when there is nothing
/// live to show (no review, another tab, or a result whose text moved on).
pub fn document_decorations(_state: &AppState, tab: &Tab) -> Vec<EditorDecoration> {
    let e = emission().get();
    let now = e.now_ms;
    if e.view.is_none() || e.tab_id != Some(tab.id) {
        return Vec::new();
    }
    let reduced = reduced_motion();
    let palette = tokens::palette();
    // Once the drain has run, the light is gone but the afterimage stays:
    // neutral ink on a ghost plate, at the positions the findings held.
    let drained = e
        .stale_ms
        .is_some_and(|stale| now >= stale + afterimage_ms(reduced));
    let mut out = Vec::new();
    for (index, slot) in e.anchors.iter().enumerate() {
        let Some(anchor) = slot else { continue };
        let heat = heat_at(&e, index, now, reduced);
        if heat <= 0.0 && !drained {
            continue;
        }
        // The twin comes first: its declarations must never win over the
        // finding's own.
        if e.pulse {
            out.push(twin_decoration(anchor));
        }
        out.push(if anchor.stated {
            stated_decoration(anchor, heat, palette)
        } else {
            inferred_decoration(anchor, heat, palette)
        });
    }
    out
}

/// A styleless copy of a finding's range: it draws nothing, and its coming
/// and going is what makes the editor's rows rebuild their spans (see
/// [`Emission::pulse`]).
///
/// Its style is transparent and comes before the finding's own, so the only
/// thing it changes is which decorations a row's spans carry — a decoration
/// with no class, style or token is dropped from a row's content, and then
/// its coming and going would change nothing.
fn twin_decoration(anchor: &Anchor) -> EditorDecoration {
    let mut decoration = EditorDecoration::default();
    decoration.range = anchor.range;
    decoration.style = Some(Style::from_decls([StyleDecl::Set(
        StyleKey::BackgroundColor,
        PropertyValue::Color(Some(Color::TRANSPARENT)),
    )]));
    decoration
}

/// Resolves every finding's anchor against `text`: the quote's words for a
/// stated finding, the anchored range otherwise.
fn resolve_anchors(view: &ReviewView, text: &str, reduced: bool) -> Vec<Option<Anchor>> {
    let questions = view.questions_detail.len();
    view.findings_anchored
        .iter()
        .enumerate()
        .map(|(index, finding)| {
            Some(Anchor {
                range: finding_range(text, finding)?,
                stated: finding.stated,
                at_ms: ignition_at(questions, index, reduced),
            })
        })
        .collect()
}

/// When the `index`-th finding ignites, in ms from the arrival: the first one
/// with the first question (their simultaneous ignition is the link), the
/// rest after the questions, in reading order, 70ms apart.
fn ignition_at(questions: usize, index: usize, reduced: bool) -> f32 {
    if reduced {
        return 0.0;
    }
    let slot = if index == 0 {
        1
    } else {
        2 + questions + index - 1
    };
    slot as f32 * STAGGER_MS
}

/// The range one finding lights up, in the editor's coordinates.
fn finding_range(text: &str, finding: &ReviewFindingView) -> Option<EditorRange> {
    let location = finding.location?;
    let (start, end) = anchored_bytes(text, location)?;
    // A stated finding's text is the source's own words: light exactly them
    // inside the anchored range (the core expands the anchor to the
    // paragraph, so the quote is found within). Without a locatable quote,
    // the anchored range itself is the light.
    if finding.stated
        && !finding.text.is_empty()
        && let Some(at) = text.get(start..end).and_then(|hay| hay.find(&finding.text))
    {
        let quote = start + at;
        return positions(text, quote, quote + finding.text.len());
    }
    positions(text, start, end)
}

/// The byte span a location anchors: the bytes themselves, or the lines'
/// bytes (from the start of the first to the end of the last).
fn anchored_bytes(text: &str, location: SourceLocation) -> Option<(usize, usize)> {
    let len = text.len();
    let floor = |byte: u64| {
        let mut byte = (byte as usize).min(len);
        while byte > 0 && !text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    };
    match location {
        SourceLocation::ByteRange { start, end } => {
            let start = floor(start);
            let end = floor(end).max(start);
            Some((start, end))
        }
        SourceLocation::LineRange { start, end } => {
            if start == 0 || start > end {
                return None;
            }
            let first = line_start_byte(text, start - 1)?;
            let last = line_start_byte(text, end - 1)?;
            let last_end = text[last..].find('\n').map_or(len, |at| last + at);
            Some((first, last_end.max(first)))
        }
    }
}

/// The byte the `line`-th line (from 0) starts at, when the text has it.
fn line_start_byte(text: &str, line: u64) -> Option<usize> {
    let mut at = 0usize;
    for _ in 0..line {
        at += text.get(at..)?.find('\n')? + 1;
    }
    Some(at.min(text.len()))
}

/// The 0-based line a byte sits on.
fn line_of(text: &str, byte: usize) -> u64 {
    let byte = byte.min(text.len());
    text[..byte].matches('\n').count() as u64
}

/// The editor range from byte `start` to byte `end` (`end` not included).
fn positions(text: &str, start: usize, end: usize) -> Option<EditorRange> {
    if start >= end || start >= text.len() {
        return None;
    }
    Some(EditorRange::new(
        position_of(text, start),
        position_of(text, end.min(text.len())),
    ))
}

/// The editor position of a byte: its line and its UTF-16 column, as the
/// editor counts columns.
fn position_of(text: &str, byte: usize) -> EditorPosition {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let column = before[line_start..].encode_utf16().count() as u32;
    EditorPosition::new(line, column)
}

// ─── the heat, as numbers ────────────────────────────────────────────────

/// The rise to HOT.
fn ignition_ms(reduced: bool) -> f32 {
    if reduced {
        REDUCED_CROSSFADE_MS
    } else {
        IGNITE_MS
    }
}

/// The cooling to ember.
fn cooling_ms(reduced: bool) -> f32 {
    if reduced {
        REDUCED_CROSSFADE_MS
    } else {
        COOL_MS
    }
}

/// The drain to the afterimage.
fn afterimage_ms(reduced: bool) -> f32 {
    if reduced {
        REDUCED_CROSSFADE_MS
    } else {
        AFTERIMAGE_MS
    }
}

/// `e`'s clock, in ms from its arrival.
fn elapsed_ms(e: &Emission) -> f32 {
    e.arrived.map_or(0.0, |arrived| {
        now().duration_since(arrived).as_millis() as f32
    })
}

/// The heat of finding `index` at `now`: `1.0` HOT, [`EMBER`] cooled,
/// `0.0` not emitting.
fn heat_at(e: &Emission, index: usize, now: f32, reduced: bool) -> f32 {
    let Some(anchor) = e.anchors.get(index).and_then(Option::as_ref) else {
        return 0.0;
    };
    let live = if e.focus == Some(index) {
        rise(now - e.focus_ms, reduced)
    } else if let Some(since) = e.cooling_since.get(index).copied().flatten() {
        // It was HOT and lost the focus: it cools from there, not from the
        // arrival's own curve.
        cooling(now - since, reduced)
    } else if now < anchor.at_ms {
        0.0
    } else {
        cooling(now - anchor.at_ms, reduced)
    };
    match e.stale_ms {
        Some(stale) => {
            let drain = ((now - stale) / afterimage_ms(reduced)).clamp(0.0, 1.0);
            live * (1.0 - drain)
        }
        None => live,
    }
}

/// The rise to `1.0`.
fn rise(elapsed: f32, reduced: bool) -> f32 {
    if reduced {
        (elapsed / REDUCED_CROSSFADE_MS).clamp(0.0, 1.0)
    } else {
        (elapsed / IGNITE_MS).clamp(0.0, 1.0)
    }
}

/// The ignition and its cooling to [`EMBER`]: [`rise`] for the first
/// [`IGNITE_MS`], then the same sampled curve [`Heat`] plays — the
/// exponential `e^(-t/420ms)`, normalised over [`COOL_MS`] so it lands
/// exactly on [`EMBER`] — because the column and the field must cool
/// together.
fn cooling(elapsed: f32, reduced: bool) -> f32 {
    if elapsed < ignition_ms(reduced) {
        return rise(elapsed, reduced);
    }
    if reduced {
        return EMBER;
    }
    let cooled = normalized_cool(elapsed - IGNITE_MS);
    1.0 - (1.0 - EMBER) * cooled
}

/// The cooling curve, normalised to reach exactly `1.0` at [`COOL_MS`]: the
/// shape of `nsh-base`'s `COOL_STOPS`, whose samples [`Heat`] plays.
fn normalized_cool(elapsed: f32) -> f32 {
    let t = elapsed.clamp(0.0, COOL_MS) / COOL_TAU_MS;
    let span = COOL_MS / COOL_TAU_MS;
    (1.0 - (-t).exp()) / (1.0 - (-span).exp())
}

/// A heat as its two levels: how much it emits (`0` nothing, `1` emitting)
/// and its temperature among emitters (`0` ember, `1` HOT).
fn levels(heat: f32) -> (f32, f32) {
    let heat = heat.clamp(0.0, 1.0);
    let live = (heat / EMBER).clamp(0.0, 1.0);
    let temperature = ((heat - EMBER) / (1.0 - EMBER)).clamp(0.0, 1.0);
    (live, temperature)
}

/// Keeps the stepped clock running while the column moves.
fn ensure_ticker() {
    let running = ticker_running();
    if running.get_untracked() {
        return;
    }
    running.set(true);
    Owner::detached().run(tick);
}

/// One step of the clock: repaints the decorations, then either waits for
/// the next step or stops.
fn tick() {
    let signal = emission();
    let e = signal.get_untracked();
    if e.view.is_none() {
        ticker_step().set(None);
        ticker_running().set(false);
        return;
    }
    let at = elapsed_ms(&e);
    let settled = !moving(&e, at);
    let appearance = tokens::palette().is_dark;
    // A repaint happens for a moving column, and once for a new appearance:
    // a settled column never repaints.
    if !settled || appearance != e.appearance {
        signal.update(|e| {
            e.appearance = appearance;
            e.pulse = !e.pulse;
            e.now_ms = at;
        });
    }
    let step = set_timeout(
        Duration::from_millis(if settled { WATCH_MS } else { TICK_MS }),
        tick,
    );
    // Replacing the handle drops the step that just ran (its timer is
    // spent) and keeps this one alive under the detached owner.
    ticker_step().set(Some(step));
}

/// Whether anything in the column is still changing at `now`.
fn moving(e: &Emission, now: f32) -> bool {
    e.view.is_some() && now < e.motion_until_ms
}

// ─── the colours ─────────────────────────────────────────────────────────

/// `from` mixed into `to` by `t`, channels and alpha both.
fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0) as f64;
    let (r1, g1, b1, a1) = from.to_f64();
    let (r2, g2, b2, a2) = to.to_f64();
    let channel = |a: f64, b: f64| ((a + (b - a) * t) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::rgba(
        channel(r1, r2),
        channel(g1, g2),
        channel(b1, b2),
        channel(a1, a2),
    )
}

fn alpha(color: Color) -> f32 {
    color.to_f64().3 as f32
}

/// `top` composited over `bottom`, as one colour: a decoration carries one
/// background, so the plate and its tint have to be flattened together.
fn over(bottom: Color, top: Color) -> Color {
    let (br, bg, bb, ba) = bottom.to_f64();
    let (tr, tg, tb, ta) = top.to_f64();
    let out_a = ta + ba * (1.0 - ta);
    if out_a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let channel = |b: f64, t: f64| {
        (((t * ta + b * ba * (1.0 - ta)) / out_a) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::rgba(
        channel(br, tr),
        channel(bg, tg),
        channel(bb, tb),
        (out_a * 255.0).round() as u8,
    )
}

/// The cooled stated word's ink: ember mixed into body ink (the spec's 35%
/// in dark, 60% in light).
fn ember_ink(palette: &Palette) -> Color {
    let t = if palette.is_dark { 0.35 } else { 0.60 };
    mix(palette.ink, palette.ember, t)
}

/// The glyph colour of a stated finding at `heat`: ink, through the cooled
/// ember ink, to HOT — and, once drained, the afterimage's neutral ink.
fn styled_glyph(heat: f32, palette: &Palette) -> Color {
    let (live, temperature) = levels(heat);
    let base = mix(palette.afterimage_ink, ember_ink(palette), live);
    mix(
        base,
        palette.hot,
        if live >= 1.0 { temperature } else { 0.0 },
    )
}

/// The plate under a stated finding: the emitter plate while it emits, the
/// afterimage's ghost plate once drained.
fn styled_plate(heat: f32, palette: &Palette) -> Color {
    let (live, _) = levels(heat);
    let lit = if palette.is_dark {
        Color::TRANSPARENT
    } else {
        palette.emitter_plate
    };
    mix(palette.afterimage_plate, lit, live)
}

/// The tight under-bloom's colour: the ember and hot blooms mixed by
/// temperature, its alpha scaled by how much the finding emits.
fn styled_bloom(heat: f32, palette: &Palette) -> Color {
    let (live, temperature) = levels(heat);
    let base = mix(palette.ember_bloom, palette.hot_bloom, temperature);
    with_alpha(base, alpha(base) * live)
}

/// The 2px tick's colour: ember to HOT by temperature, fading with the
/// emission.
fn styled_tick(heat: f32, palette: &Palette) -> Color {
    let (live, temperature) = levels(heat);
    with_alpha(mix(palette.ember, palette.hot, temperature), live)
}

/// The haze beside an inferred finding: sea-glass at the resting alpha, up
/// by three quarters when the finding is HOT, and the afterimage's ghost
/// plate once the emission has drained.
fn styled_haze(heat: f32, palette: &Palette) -> Color {
    let (live, temperature) = levels(heat);
    let haze = with_alpha(
        palette.haze,
        alpha(palette.haze) * live * (1.0 + 0.75 * temperature),
    );
    mix(palette.afterimage_plate, haze, live)
}

/// The 2px edge of an inferred finding.
fn styled_edge(heat: f32, palette: &Palette) -> Color {
    let (live, temperature) = levels(heat);
    with_alpha(
        palette.haze_edge,
        alpha(palette.haze_edge) * live * (1.0 + 0.67 * temperature),
    )
}

/// A stated finding's decoration: the glyphs themselves emit.
///
/// The under-bloom is one flat tint composited over the emitter plate: a
/// decoration carries declarations, and a gradient is not one of them (the
/// spec's stated fallback — "the bloom becomes an inline text background
/// tint from the decoration").
fn stated_decoration(anchor: &Anchor, heat: f32, palette: &Palette) -> EditorDecoration {
    let ground = over(styled_plate(heat, palette), styled_bloom(heat, palette));
    let mut decls = vec![StyleDecl::Set(
        StyleKey::Color,
        PropertyValue::Color(Some(styled_glyph(heat, palette))),
    )];
    if alpha(ground) > 0.0 {
        decls.push(StyleDecl::Set(
            StyleKey::BackgroundColor,
            PropertyValue::Color(Some(ground)),
        ));
    }
    let mut decoration = EditorDecoration::default();
    decoration.range = anchor.range;
    decoration.style = Some(Style::from_decls(decls));
    decoration.gutter = Some(tick_mark(anchor, heat, palette));
    decoration
}

/// An inferred finding's decoration: the air beside the words emits, with
/// the 2px edge, while the glyphs stay ink.
fn inferred_decoration(anchor: &Anchor, heat: f32, palette: &Palette) -> EditorDecoration {
    // Drained: no phosphor, a flat ghost plate; live: the phosphor falls off
    // over the line as one tint.
    let ground = if levels(heat).0 <= 0.0 {
        palette.afterimage_plate
    } else {
        styled_haze(heat, palette)
    };
    let mut decoration = EditorDecoration::default();
    decoration.range = anchor.range;
    decoration.line_style = Some(Style::from_decls([StyleDecl::Set(
        StyleKey::BackgroundColor,
        PropertyValue::Color(Some(ground)),
    )]));
    decoration.gutter = Some(edge_mark(styled_edge(heat, palette)));
    decoration
}

/// The 2px heat tick in the gutter, beside the lines the finding covers.
fn tick_mark(_anchor: &Anchor, heat: f32, palette: &Palette) -> EditorGutterMark {
    edge_mark(styled_tick(heat, palette))
}

/// A bar in the gutter, beside the lines a finding covers: the mark fills the
/// editor's decoration strip (a decoration cannot carry a width of its own).
fn edge_mark(color: Color) -> EditorGutterMark {
    let mut mark = EditorGutterMark::default();
    mark.placement = EditorGutterPlacement::Lines;
    mark.style = Some(Style::from_decls([StyleDecl::Set(
        StyleKey::BackgroundColor,
        PropertyValue::Color(Some(color)),
    )]));
    mark
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tokens;

    /// A finding anchored to exact bytes.
    fn stated(text: &str, at: u64, len: u64) -> ReviewFindingView {
        ReviewFindingView {
            stated: true,
            text: text.to_owned(),
            location: Some(SourceLocation::bytes(at, at + len)),
        }
    }

    #[test]
    fn byte_positions_count_lines_and_utf16_columns() {
        let text = "one\ntw\u{00e9}\u{4e2d}\nthree";
        // 'o' of "one".
        assert_eq!(position_of(text, 0), EditorPosition::new(0, 0));
        // 't' of "two" (line 1, column 0).
        assert_eq!(position_of(text, 4), EditorPosition::new(1, 0));
        // Past the CJK glyph: two UTF-16 units, one each.
        let after = 4 + "tw\u{00e9}\u{4e2d}".len();
        assert_eq!(position_of(text, after), EditorPosition::new(1, 4));
        // 't' of "three".
        assert_eq!(
            position_of(text, "one\ntw\u{00e9}\u{4e2d}\n".len()),
            EditorPosition::new(2, 0)
        );
    }

    #[test]
    fn a_stated_finding_lights_its_quote_inside_its_paragraph_anchor() {
        let text = "# Plan\n\nFirst sentence. Second sentence.\n";
        let quote = "Second sentence.";
        let paragraph = text.find("First").unwrap();
        let finding = stated(
            quote,
            paragraph as u64,
            "First sentence. Second sentence.".len() as u64,
        );
        let range = finding_range(text, &finding).expect("a range");
        assert_eq!(
            range.start,
            EditorPosition::new(2, "First sentence. ".len() as u32)
        );
        assert_eq!(
            range.end,
            EditorPosition::new(2, "First sentence. Second sentence.".len() as u32)
        );
    }

    #[test]
    fn a_line_anchor_spans_its_lines_and_names_them() {
        let text = "a\nb\nc\nd\n";
        let finding = ReviewFindingView {
            stated: false,
            text: "because the middle lines say it".to_owned(),
            location: Some(SourceLocation::lines(2, 3)),
        };
        let range = finding_range(text, &finding).expect("a range");
        // From the first line's start to the last line's content's end.
        assert_eq!(range.start, EditorPosition::new(1, 0));
        assert_eq!(range.end, EditorPosition::new(2, 1));
        assert_eq!(finding_lines(&finding, text), Some((2, 3)));
    }

    #[test]
    fn the_heat_rises_cools_holds_and_drains() {
        let view = ReviewView {
            findings_anchored: vec![stated("q", 0, 1), stated("q", 0, 1)],
            ..ReviewView::default()
        };
        let text = "q";
        let mut e = Emission {
            anchors: resolve_anchors(&view, text, false),
            view: Some(view),
            tab_id: Some(1),
            arrived: Some(now()),
            focus: None,
            focus_ms: 0.0,
            cooling_since: vec![None, None],
            stale_ms: None,
            motion_until_ms: 0.0,
            appearance: true,
            pulse: false,
            now_ms: 0.0,
        };
        // Before its slot, nothing emits; at the rise it reaches HOT.
        assert_eq!(heat_at(&e, 0, 0.0, false), 0.0);
        let first = e.anchors[0].as_ref().expect("an anchor").at_ms;
        assert!((heat_at(&e, 0, first + IGNITE_MS, false) - 1.0).abs() < 0.001);
        // It cools toward EMBER and stays there.
        let second = e.anchors[1].as_ref().expect("an anchor").at_ms;
        let cooled = heat_at(&e, 1, second + IGNITE_MS + COOL_MS, false);
        assert!((cooled - EMBER).abs() < 0.01, "cooled to {cooled}");
        // The focused finding holds HOT.
        e.focus = Some(1);
        e.focus_ms = 1000.0;
        assert_eq!(heat_at(&e, 1, 1000.0 + IGNITE_MS + COOL_MS, false), 1.0);
        // The one that lost the focus cools from the moment it did.
        e.cooling_since[0] = Some(1000.0);
        let losing = heat_at(&e, 0, 1000.0 + IGNITE_MS + COOL_TAU_MS, false);
        assert!(losing < 0.5 && losing > EMBER, "cooling from hot: {losing}");
        // Stale drains everything to the afterimage.
        e.stale_ms = Some(2000.0);
        assert_eq!(heat_at(&e, 1, 2000.0 + AFTERIMAGE_MS + 1.0, false), 0.0);
        // Reduced motion steps to ember at once.
        e.stale_ms = None;
        e.focus = None;
        let stepped = heat_at(&e, 1, second + REDUCED_CROSSFADE_MS + 1.0, true);
        assert_eq!(stepped, EMBER);
    }

    #[test]
    fn a_drained_stated_finding_is_neutral_ink_on_the_ghost_plate() {
        for palette in [&tokens::DARK, &tokens::LIGHT] {
            assert_eq!(styled_glyph(0.0, palette), palette.afterimage_ink);
            assert_eq!(styled_plate(0.0, palette), palette.afterimage_plate);
            assert_eq!(alpha(styled_bloom(0.0, palette)), 0.0);
            assert_eq!(alpha(styled_tick(0.0, palette)), 0.0);
            // At rest it is ember, and hot is hot.
            let ember_glyph = styled_glyph(EMBER, palette);
            assert_eq!(ember_glyph, ember_ink(palette));
            assert_eq!(styled_glyph(1.0, palette), palette.hot);
            // The resting bloom is the palette's ember bloom.
            assert_eq!(styled_bloom(EMBER, palette), palette.ember_bloom);
            assert_eq!(styled_tick(EMBER, palette), palette.ember);
        }
    }

    #[test]
    fn an_inferred_finding_hazes_the_air_and_keeps_the_glyphs_ink() {
        for palette in [&tokens::DARK, &tokens::LIGHT] {
            // At rest: the palette's haze, no glyph style at all.
            assert_eq!(
                alpha(styled_haze(EMBER, palette)),
                alpha(palette.haze),
                "the resting haze is the palette's"
            );
            assert!(
                alpha(styled_haze(1.0, palette)) > alpha(palette.haze),
                "HOT air hazes more"
            );
            let anchor = Anchor {
                range: EditorRange::new(EditorPosition::new(0, 0), EditorPosition::new(0, 1)),
                stated: false,
                at_ms: 0.0,
            };
            let decoration = inferred_decoration(&anchor, 1.0, palette);
            assert!(decoration.style.is_none(), "the glyphs stay ink");
            assert!(decoration.line_style.is_some(), "the line emits");
            assert!(decoration.gutter.is_some(), "with its edge");
        }
    }

    #[test]
    fn a_stated_decoration_carries_the_glyph_plate_bloom_and_tick() {
        for palette in [&tokens::DARK, &tokens::LIGHT] {
            let anchor = Anchor {
                range: EditorRange::new(EditorPosition::new(0, 0), EditorPosition::new(0, 1)),
                stated: true,
                at_ms: 0.0,
            };
            let hot = stated_decoration(&anchor, 1.0, palette);
            assert!(hot.style.is_some() && hot.gutter.is_some());
            assert!(hot.line_style.is_none(), "stated light is on the words");
            // The editor applies a decoration's *declarations*: a style the
            // editor cannot read is a style that never shows.
            assert!(
                !hot.style.as_ref().expect("a style").decls().is_empty(),
                "the span style carries declarations"
            );
            assert!(
                !hot.gutter
                    .as_ref()
                    .expect("a mark")
                    .style
                    .as_ref()
                    .expect("a mark style")
                    .decls()
                    .is_empty(),
                "the tick carries declarations"
            );
            let inferred = inferred_decoration(&anchor, 1.0, palette);
            assert!(
                !inferred
                    .line_style
                    .as_ref()
                    .expect("a line style")
                    .decls()
                    .is_empty(),
                "the haze carries declarations"
            );
            let drained = stated_decoration(&anchor, 0.0, palette);
            assert!(drained.style.is_some(), "the afterimage stays readable");
            assert!(drained.gutter.is_some());
        }
    }

    #[test]
    fn a_twin_decoration_draws_nothing_but_carries_a_style() {
        // The editor drops a decoration with no class, style or token from a
        // row's content, and a row's spans are only rebuilt when that content
        // changes: the twin has to carry a style, and it must draw nothing —
        // it comes before the finding's own decoration, whose declarations
        // then win.
        let anchor = Anchor {
            range: EditorRange::new(EditorPosition::new(0, 0), EditorPosition::new(0, 1)),
            stated: true,
            at_ms: 0.0,
        };
        let twin = twin_decoration(&anchor);
        assert_eq!(twin.range, anchor.range);
        assert!(twin.class.is_none() && twin.line_style.is_none() && twin.gutter.is_none());
        let decls = twin.style.as_ref().expect("a style").decls();
        assert_eq!(decls.len(), 1, "one declaration, and it is transparent");
        assert_eq!(
            decls[0],
            StyleDecl::Set(
                StyleKey::BackgroundColor,
                PropertyValue::Color(Some(Color::TRANSPARENT))
            )
        );
    }
}
