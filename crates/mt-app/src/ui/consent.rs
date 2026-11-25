//! Owner lane: consent.
//!
//! The hold-to-send card: the one place in the app that floats, and the one
//! thing that ever leaves the device.
//!
//! Contract: [`is_open`] says whether a consent is waiting (review, revision);
//! [`card`] draws the card *inside* the shell's overlay — the shell owns the
//! overlay layer and the room's dimming, the card owns its own size, its
//! shadow (the app's only one) and the 4px rise.
//!
//! Trust rules this file keeps:
//!
//! * the card is built from the manifest the state froze when it bound the
//!   request — the endpoint and the items that leave with their own bytes —
//!   and when no manifest exists it shows the state's own disclosure text
//!   verbatim, never summarized;
//! * the body follows that manifest: a scope switch replaces it, and the rows,
//!   the total and the hold are re-made for the binding that is now waiting;
//! * the rows add up: the content total prints exactly the sum of the rows
//!   above it, and the request's own framing is disclosed below it as *not
//!   counted*: nothing here measures the wire request, so nothing here prints
//!   a figure for it;
//! * the card takes the keyboard focus when it opens (Cancel first, a quiet
//!   action), and Esc always cancels; `Enter`/`Space` charge the hold only on
//!   the focused Send control — anywhere else they cancel the card — and a
//!   held `Ctrl+Enter` charges the same hold anywhere in the card; a tap
//!   alone authorizes nothing;
//! * a send needs a deliberate hold: a press-and-release, a keyboard tap, an
//!   auto-repeat or a handler's second hand never authorize anything, and each
//!   consent is single-use;
//! * the `Ctrl+Enter` that opened the card never charges it, however long it
//!   is held: its `KeyDown` reached the focus before the card existed, and
//!   the auto-repeats that follow carry `repeat` and are ignored; only a
//!   fresh press, after a release, charges ([`Hold::key_down`]);
//! * a rebuilt body drops what went with the old controls: the focus flag and
//!   any press or key charge in flight, whose release can never arrive;
//! * a loopback HTTP endpoint keeps its filament and row underlines plain grey
//!   and reads `HTTP (not encrypted)`.
//!
//! The authorization is one continuous hold of [`motion::FILAMENT_CHARGE_MS`]:
//! releasing, blurring, window deactivation or cancelling resets it to zero,
//! and a new press starts from zero — the charge never resumes. One input
//! owns a hold: the pointer, `Ctrl+Enter`, or `Enter`/`Space` on the focused
//! Send control. A press through another path, or a fresh press on the same
//! path, cancels the charge and its completion timer. A keyboard hold is
//! measured only from the `KeyDown`/`KeyUp` events nsh delivers to the
//! focused card ([`Hold::key_down`], [`Hold::key_up`]); a release, a fresh
//! press, a blur, a window deactivation or an unmount cancels it. Nothing is
//! read from the platform's key state or the foreground window, so the
//! pointer hold works on every platform and the card's own focus is the one
//! authority. The card captures `Enter`, `Space` and their shifted and
//! `Ctrl+Enter` forms so the platform never presses a control with them.
//!
//! The body is a `dynamic` region and `is_open` polls only the two consent
//! strings: the shell's `when` re-makes its branch when *that* flips, never
//! when a manifest inside it is replaced.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use mt_core::{
    model::{
        EndpointIdentity, EndpointLocation, OutboundScopeKind, ProxyDisclosure, TransportEncryption,
    },
    settings::Language,
};
use nsh_core::element::desktop::window_state;
use nsh_core::keymap::Keystroke;
use nsh_core::platform::Lifecycle;
use nsh_core::{Color, TransformKind, element::AnimatedValue, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, OutboundConsent, OutboundItem, OutboundOrigin, ReviewScopeChoice},
    ui::{motion, prims, review, titlebar, tokens},
};

const FILAMENT_H: f32 = 2.0;
/// The row height the height rule counts with (type plus its gap).
const ROW_H: f32 = 22.0;
/// What the card may take of the window, and the chrome around its rows.
const CARD_MAX_FRACTION: f32 = 0.80;
const CARD_CHROME_H: f32 = 320.0;
/// The headline's size and line: a step above the body, so the question is
/// read first.
const HEADLINE_SIZE: f32 = 16.0;
const HEADLINE_LINE: f32 = 24.0;
/// The definition list's term column (`To`, `Transport`, `API`, `Proxy`,
/// `Scope`).
const TERM_W: f32 = 96.0;

/// A watched key of a keyboard hold, as the card's `KeyDown`/`KeyUp`
/// handlers classify the event's logical key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRole {
    Enter,
    Space,
    /// A modifier the hold watches: a fresh press cancels the charge, and
    /// its release ends a `Ctrl+Enter` hold.
    Modifier,
    Other,
}

/// The watched key of a raw event's logical key. Windows reports Space as
/// `" "` and the modifier as `Control`; other backends spell
/// `"Space"`/`"ControlLeft"`. macOS reports Command's press and its release
/// as `Meta`/`MetaLeft`/`MetaRight` (nsh-platform-macos-0.6.1
/// `src/keyboard.rs`), so releasing Command ends the hold as releasing
/// Control does.
fn role_of(key: &str) -> KeyRole {
    if key == "Enter" || key == "NumpadEnter" {
        KeyRole::Enter
    } else if key == " " || key == "Space" {
        KeyRole::Space
    } else if key == "Control"
        || key.starts_with("Control")
        || key == "Meta"
        || key.starts_with("Meta")
        || key.starts_with("Super")
    {
        KeyRole::Modifier
    } else {
        KeyRole::Other
    }
}

/// The keys the card takes from the platform while it is open: `Enter`,
/// `Space` and their shifted and `Ctrl+Enter` forms. A capture is a mounted
/// rule, not a listener: the key still reaches the card's `on_key_down`, and
/// the platform's own defaults (Return and Space pressing the focused
/// control) never run.
pub(crate) fn key_capture() -> KeyCapture {
    let apple = apple_keys();
    let mut strokes = Vec::new();
    let mut bind = |keys: &str| {
        if let Ok(stroke) = Keystroke::parse_with(keys, apple) {
            strokes.push(stroke);
        }
    };
    for keys in [
        "enter",
        "shift+enter",
        "space",
        "shift+space",
        "mod+enter",
        "tab",
        "shift+tab",
    ] {
        bind(keys);
    }
    if apple {
        // The hint names Ctrl even on an Apple keyboard, where `mod` is
        // Command: both chords charge.
        bind("ctrl+enter");
    }
    KeyCapture::keys(strokes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsentKind {
    Review,
    Revision,
    Translation,
}

/// Whether a consent is waiting: the shell dims the room and shows the card.
pub fn is_open(state: &AppState) -> bool {
    kind_of(state).is_some()
}

pub(crate) fn kind_of(state: &AppState) -> Option<ConsentKind> {
    if state.review_consent.get().is_some() {
        Some(ConsentKind::Review)
    } else if state.revision_consent.get().is_some() {
        Some(ConsentKind::Revision)
    } else if state.translation_consent.get().is_some() {
        Some(ConsentKind::Translation)
    } else {
        None
    }
}

/// [`kind_of`] without subscribing the caller: the card itself must not be
/// rebuilt by the consent it is already drawing (a rebuild would drop the
/// spent hold's `Sent` fade).
fn kind_of_untracked(state: &AppState) -> Option<ConsentKind> {
    if state.review_consent.get_untracked().is_some() {
        Some(ConsentKind::Review)
    } else if state.revision_consent.get_untracked().is_some() {
        Some(ConsentKind::Revision)
    } else if state.translation_consent.get_untracked().is_some() {
        Some(ConsentKind::Translation)
    } else {
        None
    }
}

/// The consent the card shows.
struct Waiting {
    kind: ConsentKind,
    consent: Option<OutboundConsent>,
    fallback: SharedString,
    offers_selection: bool,
    offers_context: bool,
}

fn waiting(state: &AppState) -> Waiting {
    match kind_of(state) {
        Some(ConsentKind::Revision) => Waiting {
            kind: ConsentKind::Revision,
            consent: state.revision_manifest.get(),
            fallback: state.revision_consent.get().unwrap_or_default(),
            offers_selection: false,
            offers_context: false,
        },
        Some(ConsentKind::Translation) => Waiting {
            kind: ConsentKind::Translation,
            consent: state.translation_manifest.get(),
            fallback: state.translation_consent.get().unwrap_or_default(),
            offers_selection: false,
            offers_context: false,
        },
        _ => Waiting {
            kind: ConsentKind::Review,
            consent: state.review_manifest.get(),
            fallback: state.review_consent.get().unwrap_or_default(),
            offers_selection: state.review_consent_offers_selection(),
            offers_context: state.review_consent_offers_context(),
        },
    }
}

/// The pending one-shot focus of an open card, kept alive until it fires: an
/// effect runs before the commit that carries the card's node is in, so its
/// ref reaches it only at the next step.
fn card_focus_step() -> Signal<Option<Spawned<()>>> {
    global::<Option<Spawned<()>>>()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RingStop {
    Card,
    ScopeSelection,
    ScopeDocument,
    ScopeContext,
    Cancel,
    Send,
}

impl RingStop {
    fn index(self) -> Option<usize> {
        match self {
            RingStop::Card => None,
            RingStop::ScopeSelection => Some(0),
            RingStop::ScopeDocument => Some(1),
            RingStop::ScopeContext => Some(2),
            RingStop::Cancel => Some(3),
            RingStop::Send => Some(4),
        }
    }
}

// The card's Tab ring while it is modal: the card itself plus the controls
// of the body built now. `tab`/`shift+tab` are captured by `key_capture` and
// move the focus here, so the platforms' own traversal — the whole window
// when no native modal is presented — never leaves the card. A stop with no
// node (a scope the manifest does not offer) is skipped, and Tab from the
// card reaches the first built control.
struct CardFocus {
    card: ElementRef,
    scope_selection: ElementRef,
    scope_document: ElementRef,
    scope_context: ElementRef,
    cancel: ElementRef,
    send: ElementRef,
    landed: Cell<Option<usize>>,
}

impl CardFocus {
    const RING: [RingStop; 5] = [
        RingStop::ScopeSelection,
        RingStop::ScopeDocument,
        RingStop::ScopeContext,
        RingStop::Cancel,
        RingStop::Send,
    ];

    fn new() -> Self {
        Self {
            card: element_ref(),
            scope_selection: element_ref(),
            scope_document: element_ref(),
            scope_context: element_ref(),
            cancel: element_ref(),
            send: element_ref(),
            landed: Cell::new(None),
        }
    }

    fn at(&self, stop: RingStop) -> &ElementRef {
        match stop {
            RingStop::Card => &self.card,
            RingStop::ScopeSelection => &self.scope_selection,
            RingStop::ScopeDocument => &self.scope_document,
            RingStop::ScopeContext => &self.scope_context,
            RingStop::Cancel => &self.cancel,
            RingStop::Send => &self.send,
        }
    }

    fn land(&self, stop: RingStop) {
        self.landed.set(stop.index());
    }

    // The next built control after the one that landed last, wrapping both
    // ways and skipping stops with no node; from the card, forward reaches
    // the first built control and back the last.
    fn step(&self, back: bool) {
        let len = Self::RING.len();
        let here = self.landed.get().filter(|index| *index < len);
        for offset in 1..=len {
            let index = match (here, back) {
                (None, false) => offset - 1,
                (None, true) => len - offset,
                (Some(here), false) => (here + offset) % len,
                (Some(here), true) => (here + len - offset) % len,
            };
            let stop = Self::RING[index];
            if self.at(stop).node().is_some() {
                self.at(stop).focus();
                self.land(stop);
                return;
            }
        }
    }
}

/// The consent card.
pub fn card(state: AppState, language: Language) -> impl IntoElement {
    // The card owns Enter, Escape and the hold: the source editor gives up
    // the focus it may still hold from the Ctrl+Enter that opened the card.
    state.blur_source_editor();
    let kind = kind_of_untracked(&state).unwrap_or(ConsentKind::Review);
    let shadow = shadow();
    // One hold for as long as the card lives: the body resets it whenever it
    // is built again.
    let hold = Hold::new(state.clone(), kind, language);
    // Esc is a keymap binding; the hold's keys are the card's own raw
    // handlers below, so they measure only what the focused card hears.
    keyboard(state.clone(), kind);
    // A card that leaves drops its charge and its completion timer with it.
    cleanup({
        let hold = hold.clone();
        move || hold.cancel_hold()
    });
    // The window leaving the foreground, or losing the keyboard focus to a
    // native sheet (the pickers open one; nsh reports it in
    // `WindowState::focused`), cancels any hold: neither a stuck key nor a
    // pointer already down may matter once the room is not ours.
    effect({
        let hold = hold.clone();
        move || {
            let window_focused = window_state().is_none_or(|state| state.focused);
            if lifecycle() != Lifecycle::Active || !window_focused {
                hold.release();
            }
        }
    });
    let ring = Rc::new(CardFocus::new());
    let body_state = state.clone();
    let body_hold = hold.clone();
    let presented = signal_with(None, SignalEquals::Never);
    effect({
        let state = state.clone();
        let hold = hold.clone();
        move || {
            if is_open(&state) {
                presented.set(Some(waiting(&state)));
            } else {
                hold.rebuild(None, 0);
            }
        }
    });
    let down = hold.clone();
    let up = hold.clone();
    let blur = hold.clone();
    let focus = ring.card;
    // The card takes the keyboard focus when it opens: Cancel stays the
    // first control Tab reaches, but the card itself owns the keys until
    // then, so `Ctrl+Enter` from before it opened can never charge it. The
    // focus fires after the commit that mounts the card (a ref reaches its
    // node only then).
    let focus_step = card_focus_step();
    effect(move || {
        focus_step.set(Some(set_timeout(Duration::from_millis(0), move || {
            focus.focus();
        })));
    });
    view()
        .test_id("consent-card")
        .w(px(tokens::CONSENT_CARD_MAX_W))
        .flex_col()
        .gap(px(tokens::SPACE_4))
        .p(px(tokens::SPACE_5))
        .focusable(true)
        .element_ref(&focus)
        .on_focus({
            let ring = ring.clone();
            move || {
                ring.land(RingStop::Card);
                #[cfg(test)]
                tests::record_focus(tests::FocusStop::Card);
            }
        })
        // The card takes the keyboard focus when it opens; these handlers
        // see the keys wherever inside the card the focus is, because the
        // events bubble to the card.
        .capture_keys(key_capture())
        .on_key_down({
            let ring = ring.clone();
            move |event: &KeyEvent| {
                if event.key == "Tab" && !event.repeat {
                    ring.step(event.modifiers.shift);
                    return;
                }
                down.key_down(event);
            }
        })
        .on_key_up(move |event: &KeyEvent| up.key_up(event))
        .on_blur(move || blur.release())
        // In dark mode the card's ground is lifted a hair off the room and a
        // hairline draws its edge: the shadow alone does not show on graphite.
        .bg(move || tokens::palette().card_ground)
        .border(px(tokens::HAIRLINE))
        .border_color(move || {
            let palette = tokens::palette();
            if palette.is_dark {
                palette.hairline
            } else {
                Color::TRANSPARENT
            }
        })
        .shadow([shadow])
        .shadow_color(AdaptiveColor::new(
            Color::rgba(0, 0, 0, 36),
            Color::rgba(0, 0, 0, 115),
        ))
        .child(
            text(match kind {
                ConsentKind::Review => S::HeadlineReview.text(language),
                ConsentKind::Revision => S::HeadlineRevision.text(language),
                ConsentKind::Translation => S::HeadlineTranslation.text(language),
            })
            .font_family(tokens::FONT_MEDIUM)
            .font_weight(tokens::WEIGHT_MEDIUM)
            .text_size(px(HEADLINE_SIZE))
            .line_height(px(HEADLINE_LINE))
            .text_color(move || tokens::palette().ink),
        )
        .child(dynamic(move || {
            presented.with(|waiting| {
                waiting
                    .as_ref()
                    .map(|waiting| card_body(&body_state, language, &body_hold, waiting, &ring))
            })
        }))
}

/// The disclosure and hold use the same manifest binding.
fn card_body(
    state: &AppState,
    language: Language,
    hold: &Hold,
    waiting: &Waiting,
    ring: &Rc<CardFocus>,
) -> AnyElement {
    let rows = waiting
        .consent
        .as_ref()
        .map(|consent| rows(consent, language));
    let total = rows.as_ref().map_or(0, |rows| rows.total);
    hold.rebuild(waiting.consent.as_ref(), total);
    match waiting.consent.as_ref() {
        Some(consent) => view()
            .flex_col()
            .gap(px(tokens::SPACE_4))
            .child(disclosure_body(
                state,
                language,
                hold,
                waiting,
                consent,
                rows.as_ref().expect("a manifest has its rows"),
                ring,
            ))
            .child(buttons(
                state.clone(),
                waiting.kind,
                language,
                hold.clone(),
                total,
                ring.clone(),
            ))
            .into_element(),
        None => text(waiting.fallback.clone())
            .font_family(tokens::FONT_UI)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink_rest)
            .into_element(),
    }
}

/// The card's body: destination, scope, the rows that leave, the content
/// total, the framed remainder and the boundary statement.
fn disclosure_body(
    state: &AppState,
    language: Language,
    hold: &Hold,
    waiting: &Waiting,
    consent: &OutboundConsent,
    rows: &Rows,
    ring: &Rc<CardFocus>,
) -> impl IntoElement {
    let loopback =
        consent.disclosure.endpoint().transport().encryption() == TransportEncryption::Unencrypted;
    let scope_state = state.clone();
    view()
        .test_id("consent-disclosure")
        .flex_col()
        .gap(px(tokens::SPACE_4))
        .child(
            // The destination and the scope: one two-column list, terms at
            // resting ink, values in the source face.
            view()
                .flex_col()
                .gap(px(tokens::SPACE_1))
                .child(destination_list(language, consent.disclosure.endpoint()))
                .child(scope_row(scope_state, language, hold, waiting, ring)),
        )
        .child(
            // Every row is on the card: a long manifest scrolls in place, so
            // the scope, the total and the controls stay in view.
            scroll()
                .test_id("consent-rows-scroll")
                .max_h(px(rows_max_h(window().height.0 * CARD_MAX_FRACTION)))
                .child(rows_section(language, rows, loopback)),
        )
        .child(
            view()
                .flex_col()
                .gap(px(tokens::SPACE_2))
                .child(prims::hairline())
                .child(
                    view()
                        .test_id("consent-total")
                        .flex_row()
                        .items_center()
                        .gap(px(tokens::SPACE_2))
                        .child(view().flex_1().child(prims::rest(S::Total.text(language))))
                        .child(
                            tokens::mono_count(text(SharedString::from(bytes_label(rows.total))))
                                .text_size(px(tokens::TYPE_UI))
                                .line_height(px(tokens::TYPE_UI_LINE))
                                .text_color(move || tokens::palette().ink),
                        ),
                )
                .child(framing_section(language, loopback)),
        )
        .child(prims::rest(S::Boundary.text(language)))
}

/// The destination as a definition list: where (`To`), how (`Transport`), in
/// what wire format (`API`) and through which proxy, in that order. Each
/// value is its own text, so a long one wraps between its words.
fn destination_list(language: Language, endpoint: &EndpointIdentity) -> impl IntoElement {
    let mut list = view()
        .test_id("consent-destination")
        .flex_col()
        .gap(px(tokens::SPACE_1));
    for (term, value) in destination(language, endpoint) {
        list = list.child(
            view()
                .flex_row()
                .items_start()
                .child(term_view(term.text(language)))
                .child(
                    text(SharedString::from(value))
                        .font_family(tokens::FONT_SOURCE)
                        .text_size(px(tokens::TYPE_UI))
                        .line_height(px(tokens::TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink)
                        .flex_1()
                        .min_w(px(0.0)),
                ),
        );
    }
    list
}

/// A definition list's term, in its fixed column.
fn term_view(term: &'static str) -> impl IntoElement {
    view()
        .w(px(TERM_W))
        .flex_shrink_0()
        .child(prims::rest(term))
}

/// The scope: the words the state can bind (`Selection`, `Document`,
/// `Document + context`), the current one in full ink, each other bindable one
/// a quiet switch (choosing a scope is your action, never warm), and one that
/// cannot be bound not drawn. A switch rebinds the consent; it authorizes
/// nothing.
fn scope_row(
    state: AppState,
    language: Language,
    hold: &Hold,
    waiting: &Waiting,
    ring: &Rc<CardFocus>,
) -> impl IntoElement {
    let kind = waiting
        .consent
        .as_ref()
        .map(|consent| consent.disclosure.scope().kind());
    // Whether the binding that waits carries context: a package can carry it
    // too, and then `Document + context` is the one in force, not a switch.
    let with_context = waiting.consent.as_ref().is_some_and(|consent| {
        !consent
            .disclosure
            .scope()
            .effective_context_sources()
            .is_empty()
    });
    let current = if kind == Some(OutboundScopeKind::Selection) {
        ReviewScopeChoice::Selection
    } else if with_context {
        ReviewScopeChoice::DocumentWithContext
    } else {
        ReviewScopeChoice::Document
    };
    let option = |choice: ReviewScopeChoice,
                  stop: RingStop,
                  test_id: &'static str,
                  label: S,
                  offered: bool| {
        let label = label.text(language);
        if current == choice {
            return Some(scope_word(test_id, label));
        }
        let state = state.clone();
        let ring = ring.clone();
        offered.then(|| {
            let enabled = state.clone();
            let blur = hold.clone();
            let action = prims::quiet_action(test_id, label, move || {
                state.set_review_consent_scope(choice)
            })
            .element_ref(ring.at(stop))
            .on_blur(move || blur.release());
            let ring = ring.clone();
            let action = action.on_focus(move || {
                ring.land(stop);
                #[cfg(test)]
                tests::record_focus(tests::FocusStop::Scope);
            });
            view()
                .child(action.disabled(move || !is_open(&enabled)))
                .into_element()
        })
    };
    let options: Vec<AnyElement> = [
        option(
            ReviewScopeChoice::Selection,
            RingStop::ScopeSelection,
            "consent-scope-selection",
            S::ScopeSelection,
            waiting.offers_selection,
        ),
        option(
            ReviewScopeChoice::Document,
            RingStop::ScopeDocument,
            "consent-scope-document",
            S::ScopeDocument,
            waiting.kind != ConsentKind::Revision,
        ),
        option(
            ReviewScopeChoice::DocumentWithContext,
            RingStop::ScopeContext,
            "consent-scope-context",
            S::ScopeContext,
            waiting.offers_context,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    view()
        .test_id("consent-scope")
        .flex_row()
        .items_center()
        .child(term_view(S::Scope.text(language)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_3))
                .children(options),
        )
}

/// The scope the disclosure carries, in full ink.
fn scope_word(test_id: &'static str, label: &'static str) -> AnyElement {
    view()
        .test_id(test_id)
        .child(prims::ink(label))
        .into_element()
}

/// How a row is lit: outbound data is white-hot, a loopback endpoint is plain
/// grey, and what stays is unlit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Leaving,
    Loopback,
    Unlit,
}

/// The content rows that leave, then the items that stay: every item, each
/// with its path, size and reason. The caller bounds the section's height.
fn rows_section(language: Language, rows: &Rows, loopback: bool) -> impl IntoElement {
    let tone = tone_of(loopback);
    let mut section = view()
        .test_id("consent-rows")
        .flex_col()
        .w_full()
        .gap(px(tokens::SPACE_1));
    if !rows.leaving.is_empty() {
        section = section.child(prims::rest(S::LeavesThisDevice.text(language)));
        for row in &rows.leaving {
            section = section.child(row_view(row, tone));
        }
    }
    if !rows.metadata.is_empty() {
        section = section.child(
            view()
                .pt(px(tokens::SPACE_2))
                .child(prims::rest(S::MetadataOnly.text(language))),
        );
        for row in &rows.metadata {
            section = section.child(row_view(row, Tone::Unlit));
        }
    }
    if !rows.stays.is_empty() {
        section = section.child(
            view()
                .pt(px(tokens::SPACE_2))
                .child(prims::rest(S::StaysHere.text(language))),
        );
        for row in &rows.stays {
            section = section.child(row_view(row, Tone::Unlit));
        }
    }
    section
}

/// What travels beyond the disclosed content: the request's own framing. It
/// carries no byte figure, because nothing here measures the request that
/// leaves — only mt-core's canonical frame, which is not the wire body.
fn framing_section(language: Language, loopback: bool) -> AnyElement {
    let row = Row {
        origin: OutboundOrigin::Document,
        label: S::RequestFraming.text(language).to_owned(),
        reason: S::FramingReason.text(language).to_owned(),
        bytes: 0,
        has_bytes: false,
    };
    view()
        .test_id("consent-framing")
        .flex_col()
        .w_full()
        .gap(px(tokens::SPACE_1))
        .child(row_view(&row, tone_of(loopback)))
        .into_element()
}

/// The colour a row that leaves is drawn in: the loopback endpoint keeps it
/// plain grey.
fn tone_of(loopback: bool) -> Tone {
    if loopback {
        Tone::Loopback
    } else {
        Tone::Leaving
    }
}

/// One manifest row: path, reason, bytes right-aligned, and the under-line —
/// white-hot as outbound data, plain grey for a loopback endpoint, ambient for
/// what stays. Only white-hot rows sit on the emitter plate: a loopback row
/// makes no heat claim and what stays behind is unlit, so both stay on the
/// room.
fn row_view(row: &Row, tone: Tone) -> impl IntoElement {
    let label = SharedString::from(row.label.clone());
    let reason = SharedString::from(row.reason.clone());
    let bytes = row.bytes;
    let has_bytes = row.has_bytes;
    let gloss = move || {
        let palette = tokens::palette();
        match tone {
            Tone::Leaving => (palette.leaving, tokens::with_alpha(palette.leaving, 0.62)),
            Tone::Loopback => (palette.loopback, tokens::with_alpha(palette.loopback, 0.55)),
            Tone::Unlit => (palette.ink_rest, palette.hairline),
        }
    };
    let test_id = match row.origin {
        OutboundOrigin::Omission | OutboundOrigin::ExcludedSource => "consent-row-stays",
        OutboundOrigin::NamedSource => "consent-row-source",
        OutboundOrigin::PackageFile => "consent-row-file",
        _ => "consent-row",
    };
    let content = || {
        view()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_2))
            .child(
                text(label.clone())
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || gloss().0),
            )
            .child(
                text(reason.clone())
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || gloss().1),
            )
            .child(view().flex_1())
            .child(
                tokens::mono_count(text(move || {
                    if has_bytes {
                        SharedString::from(bytes_label(bytes))
                    } else {
                        SharedString::from("")
                    }
                }))
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || gloss().0),
            )
    };
    let content = match tone {
        Tone::Leaving => prims::emitter_plate(content()).into_element(),
        Tone::Loopback | Tone::Unlit => content().into_element(),
    };
    view()
        .test_id(test_id)
        .flex_col()
        .gap(px(tokens::SPACE_1))
        .child(content)
        .child(
            view()
                .h(px(tokens::HAIRLINE))
                .w_full()
                .bg(move || gloss().1),
        )
}

/// The two controls, Cancel (default focus, quiet) and the hold, then the
/// hold's key as `kbd` chips.
fn buttons(
    state: AppState,
    kind: ConsentKind,
    language: Language,
    hold: Hold,
    total: u64,
    ring: Rc<CardFocus>,
) -> impl IntoElement {
    view()
        .test_id("consent-buttons")
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_3))
        .child(cancel_button(
            state,
            kind,
            language,
            hold.clone(),
            ring.clone(),
        ))
        .child(send_button(hold, language, total, ring))
        // The keyboard route that needs no focus anywhere: the card holds
        // the focus, and this hold works from anywhere inside it.
        .child(
            view()
                .test_id("consent-key-hint")
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_1))
                .child(prims::rest(S::OrHold.text(language)))
                .child(prims::kbd("Ctrl"))
                .child(prims::rest("+"))
                .child(prims::kbd("Enter")),
        )
}

fn cancel_button(
    state: AppState,
    kind: ConsentKind,
    language: Language,
    hold: Hold,
    ring: Rc<CardFocus>,
) -> impl IntoElement {
    let enabled = state.clone();
    let blur = hold.clone();
    view().test_id("consent-cancel").child(cancel_action(
        i18n::t(language, Key::Cancel),
        move || cancel(&state, kind),
        move || blur.release(),
        move || !is_open(&enabled),
        ring,
    ))
}

/// `prims::quiet_action`'s look, as the Cancel control itself: resting ink
/// that steps to full ink, underlined, on hover or keyboard focus (drawn by
/// the text, which a style on the control would not reach). The control is
/// one focusable view of its own: focus reaches it directly, and losing that
/// focus cancels a `Ctrl+Enter` charge in flight.
fn cancel_action(
    label: &'static str,
    on_press: impl Fn() + 'static,
    on_blur: impl Fn() + 'static,
    disabled: impl Fn() -> bool + 'static,
    ring: Rc<CardFocus>,
) -> AnyElement {
    let duration = prims::press_color_duration();
    let focused = ring.clone();
    let control = view()
        .test_id("consent-cancel-button")
        .flex_row()
        .items_center()
        .px(px(2.0))
        .py(px(4.0))
        .bg(Color::TRANSPARENT)
        .cursor_pointer()
        .group()
        .focusable(true)
        .disabled(disabled)
        .element_ref(ring.at(RingStop::Cancel))
        .role(AccessibilityRole::Button)
        .label(SharedString::from(label))
        .on_press(move |_: &PressEvent| on_press())
        .on_blur(on_blur)
        .on_focus(move || {
            focused.land(RingStop::Cancel);
            #[cfg(test)]
            tests::record_focus(tests::FocusStop::Cancel);
        });
    prims::press_feedback_with_duration(
        control.child(
            text(label)
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest)
                .underline_offset(px(4.0))
                .transition_colors()
                .duration(move || duration.get())
                .group_hover(|s| s.text_color(|| tokens::palette().ink).underline())
                .group_focus_visible(|s| s.text_color(|| tokens::palette().ink).underline()),
        ),
        duration,
    )
    .into_element()
}

/// Hold to send: a full [`motion::FILAMENT_CHARGE_MS`] of charge authorizes;
/// anything shorter authorizes nothing.
fn send_button(
    hold: Hold,
    language: Language,
    total: u64,
    ring: Rc<CardFocus>,
) -> impl IntoElement {
    let heat = hold.heat;
    let blur = hold.clone();
    let focused = hold.clone();
    let ring_landing = ring.clone();
    let down = hold.clone();
    let up = hold.clone();
    let tap = hold.clone();
    let label_hold = hold.clone();
    let enabled = hold.state.clone();
    prims::press_feedback(
        view()
            .test_id("consent-send")
            .w(px(tokens::CONSENT_BUTTON_W))
            .h(px(tokens::CONSENT_BUTTON_H))
            .relative()
            .flex_col()
            .items_center()
            .justify_center()
            .border(px(tokens::HAIRLINE))
            .border_color(move || tokens::palette().ink_caption)
            .focus_visible(|s| s.border_color(|| tokens::palette().ink))
            .cursor_pointer()
            .focusable(true)
            .disabled(move || !is_open(&enabled))
            .label(SharedString::from(hold_accessibility(language, total)))
            .element_ref(ring.at(RingStop::Send))
            .on_focus(move || {
                focused.focused.set(true);
                ring_landing.land(RingStop::Send);
                #[cfg(test)]
                tests::record_focus(tests::FocusStop::Hold);
            })
            // A hold must not authorize once focus leaves Send.
            .on_blur(move || {
                blur.focused.set(false);
                blur.release();
            })
            .on_press_down(move |_: &PressEvent| down.press())
            .on_press_up(move |_: &PressEvent| up.release())
            // An assistive technology's activation arrives as one whole press: the
            // press ended, so the charge cools and nothing is authorized.
            .on_press(move |_: &PressEvent| tap.release())
            .child(
                text(move || match label_hold.label.get() {
                    HoldLabel::Idle => SharedString::from(S::HoldToSend.text(language)),
                    HoldLabel::Holding => SharedString::from(S::KeepHolding.text(language)),
                    HoldLabel::StepOne => SharedString::from(S::StepOne.text(language)),
                    HoldLabel::StepTwo => SharedString::from(S::StepTwo.text(language)),
                    HoldLabel::StepThree => SharedString::from(S::Sending.text(language)),
                    HoldLabel::Released => SharedString::from(S::NothingWasSent.text(language)),
                    HoldLabel::Sent => SharedString::from(S::Sent.text(language)),
                })
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink),
            )
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w_full()
                    .h(px(FILAMENT_H))
                    .bg(move || tokens::palette().filament[0]),
            )
            .child(dynamic(move || {
                let filament_fill = view()
                    .absolute()
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w_full()
                    .h(px(FILAMENT_H))
                    .test_id("consent-filament")
                    .origin_bottom_left()
                    .bind_background(&heat.filament_glow());
                if motion::reduced_motion() {
                    filament_fill.bind_opacity(&heat)
                } else {
                    filament_fill.bind_transform(TransformKind::ScaleX, &heat)
                }
            })),
    )
}

/// The card's keymap keys: Esc always cancels, from wherever the card's
/// focus is. The hold's keys are not bindings: they are the card's own raw
/// `KeyDown`/`KeyUp` handlers ([`Hold::key_down`]), so only what the focused
/// card hears moves the charge and the keymap can never measure a key the
/// card did not receive.
fn keyboard(state: AppState, kind: ConsentKind) {
    effect(move || {
        if !is_open(&state) {
            return;
        }
        on_key_with("escape", KeyOptions::default(), {
            let state = state.clone();
            move |_| {
                if !titlebar::escape_closes_menu(&state) {
                    cancel(&state, kind);
                }
            }
        });
    });
}

/// Cancel the waiting consent, whatever it is.
fn cancel(state: &AppState, kind: ConsentKind) {
    match kind {
        ConsentKind::Revision => review::cancel_revision(state),
        ConsentKind::Review => state.cancel_review(),
        ConsentKind::Translation => state.cancel_translation(),
    }
}

// The card's Escape, for a surface whose own Escape binding fired instead of
// the card's because it mounted later (a Review or Revision field). A waiting
// menu keeps its Escape first, as it does in `keyboard`. Returns whether a
// card was waiting.
pub(crate) fn cancel_open(state: &AppState) -> bool {
    let Some(kind) = kind_of(state) else {
        return false;
    };
    if !titlebar::escape_closes_menu(state) {
        cancel(state, kind);
    }
    true
}

// ─── the manifest's rows ──────────────────────────────────────────────────

/// One row the card draws.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    origin: OutboundOrigin,
    label: String,
    reason: String,
    bytes: u64,
    /// False for a row with no byte figure (an item that carries none).
    has_bytes: bool,
}

/// The card's rows, and the total that equals the sum on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rows {
    /// The items whose content leaves, in display order.
    leaving: Vec<Row>,
    /// Package entries that leave as path, size and hash only: listed with
    /// their file size, but not part of the total — what they contribute at
    /// the boundary is request framing, which the card discloses without a
    /// figure.
    metadata: Vec<Row>,
    /// The items that stay.
    stays: Vec<Row>,
    /// The content bytes the rows above the total line sum to. Request framing
    /// is not part of it and is not counted anywhere.
    total: u64,
}

/// The rows of one consent, in the card's words: one row per item, none
/// hidden, because every file that leaves is disclosed before consent.
fn rows(consent: &OutboundConsent, language: Language) -> Rows {
    let content_bytes = consent
        .items
        .iter()
        .filter(|item| item.content_leaves)
        .fold(0_u64, |sum, item| sum.saturating_add(item.bytes));
    let leaving = consent
        .items
        .iter()
        .filter(|item| item.content_leaves)
        .map(|item| row_of(item, language))
        .collect();
    let metadata = consent
        .items
        .iter()
        .filter(|item| !item.content_leaves)
        .map(|item| row_of(item, language))
        .collect();
    let stays = consent
        .stays
        .iter()
        .map(|item| row_of(item, language))
        .collect();
    Rows {
        leaving,
        metadata,
        stays,
        total: content_bytes,
    }
}

/// One item as a row.
fn row_of(item: &OutboundItem, language: Language) -> Row {
    Row {
        origin: item.origin,
        label: item
            .path
            .clone()
            .unwrap_or_else(|| name(language, item.origin).to_owned()),
        reason: reason_of(language, item),
        bytes: item.bytes,
        has_bytes: item.bytes > 0,
    }
}

/// How tall the scrolling rows may grow in a card that may take `height`
/// points of the window: at least two rows, at most 64.
fn rows_max_h(height: f32) -> f32 {
    let rows = ((height - CARD_CHROME_H) / ROW_H).floor();
    let rows = if rows.is_finite() {
        rows.clamp(2.0, 64.0)
    } else {
        2.0
    };
    rows * ROW_H
}

/// An item's own name when it has no path.
fn name(language: Language, origin: OutboundOrigin) -> &'static str {
    match origin {
        OutboundOrigin::Document => S::DocumentName.text(language),
        OutboundOrigin::Selection => S::SelectionName.text(language),
        OutboundOrigin::NamedSource => S::NamedSourceName.text(language),
        OutboundOrigin::PackageFile => S::PackageFileName.text(language),
        OutboundOrigin::Omission => S::OmissionName.text(language),
        OutboundOrigin::ExcludedSource => S::ExcludedSourceName.text(language),
        OutboundOrigin::ReviewContext => S::ReviewContextName.text(language),
        OutboundOrigin::Answers => S::AnswersName.text(language),
    }
}

/// What the row says it is: the package's own inclusion reason when it has one,
/// else this direction's words.
fn reason_of(language: Language, item: &OutboundItem) -> String {
    if let Some(reason) = &item.reason {
        return reason.clone();
    }
    match item.origin {
        OutboundOrigin::Document => S::DocumentReason.text(language).to_owned(),
        OutboundOrigin::Selection => S::SelectionReason.text(language).to_owned(),
        OutboundOrigin::NamedSource => S::NamedSourceReason.text(language).to_owned(),
        OutboundOrigin::PackageFile => S::PackageFileReason.text(language).to_owned(),
        OutboundOrigin::Omission => S::OmissionReason.text(language).to_owned(),
        OutboundOrigin::ExcludedSource => S::ExcludedSourceReason.text(language).to_owned(),
        OutboundOrigin::ReviewContext => S::ReviewContextReason.text(language).to_owned(),
        OutboundOrigin::Answers => S::AnswersReason.text(language).to_owned(),
    }
}

/// The destination, term by term: where (`To`), how (`Transport`), in what
/// wire format (`API`), and what the transport does with proxies. Whether
/// the endpoint is local or remote and the proxy are disclosures of their
/// own, never implied.
fn destination(language: Language, endpoint: &EndpointIdentity) -> [(S, String); 4] {
    let location = match endpoint.location() {
        EndpointLocation::Local => S::LocalModel.text(language),
        EndpointLocation::Remote => S::Remote.text(language),
    };
    let transport = if endpoint.transport().encryption() == TransportEncryption::Encrypted {
        "HTTPS"
    } else {
        S::Unencrypted.text(language)
    };
    let proxy = match endpoint.transport().proxy() {
        ProxyDisclosure::MayUseConfiguredProxy => S::ProxySystem.text(language),
        ProxyDisclosure::Disabled => S::ProxyDisabled.text(language),
    };
    [
        (
            S::To,
            format!(
                "{location} \u{00B7} {}:{}",
                endpoint.host(),
                endpoint.effective_port()
            ),
        ),
        (S::Transport, transport.to_owned()),
        (
            S::Api,
            format!(
                "{} {}",
                endpoint.provider().label(),
                endpoint.api_base_path().trim_end_matches('/')
            ),
        ),
        (S::Proxy, proxy.to_owned()),
    ]
}

/// Bytes as the card writes them: `2,140 B`.
fn bytes_label(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 2);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} B")
}

/// What the hold control says it sends: the visible label stays the plain
/// invitation, and the byte figure is the content the card lists, named as
/// such (the framed request is larger, and the rows say so).
fn hold_accessibility(language: Language, bytes: u64) -> String {
    format!(
        "{} {}",
        S::HoldToSend.text(language),
        sent_bytes(language, bytes)
    )
}

/// The content figure with its unit: what the card's sum counts, as the
/// status a completed hold leaves reads it (`Sent · 110 B`).
fn sent_bytes(_language: Language, bytes: u64) -> String {
    bytes_label(bytes)
}

/// The card's shadow: the app's only one, 0 30px 80px at /.45 (dark) or /.14
/// (light). Its colour is adaptive (see the card), so it follows the theme
/// without a rebuild.
fn shadow() -> BoxShadow {
    BoxShadow {
        offset_x: Points(0.0),
        offset_y: Points(30.0),
        blur: Points(80.0),
        spread: Points(0.0),
        color: Color::TRANSPARENT,
        inset: false,
    }
}

// ─── the hold ─────────────────────────────────────────────────────────────

/// What the hold control's label says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldLabel {
    Idle,
    Holding,
    /// Reduced motion: the filament's three discrete steps.
    StepOne,
    StepTwo,
    StepThree,
    Released,
    Sent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldPath {
    Pointer,
    CtrlEnter,
    SendEnter,
    SendSpace,
}

impl HoldPath {
    fn is_keyboard(self) -> bool {
        !matches!(self, HoldPath::Pointer)
    }
}

/// The charge after holding for `ms` from `charge`.
fn charged(charge: f32, ms: f32) -> f32 {
    (charge + ms / motion::FILAMENT_CHARGE_MS).min(1.0)
}

/// The charge after `ms` with the press ended: the spec's decay to dull, tau
/// 250ms.
fn cooled(charge: f32, ms: f32) -> f32 {
    charge * (-ms / motion::FILAMENT_RELEASE_TAU_MS).exp()
}

/// Milliseconds since `at` on the engine's clock, which the hold's timers
/// also run on.
fn elapsed_ms(at: Instant) -> f32 {
    nsh_core::time::now()
        .saturating_duration_since(at)
        .as_secs_f32()
        * 1000.0
}

/// The hold time a charge still needs.
#[cfg(test)]
fn remaining_ms(charge: f32) -> f32 {
    (1.0 - charge).max(0.0) * motion::FILAMENT_CHARGE_MS
}

/// How many of reduced motion's three steps a charge has reached: 0 before the
/// first third, 1 and 2 in the middle, and never 3 (the charge completing is
/// the third step, drawn as `Sending`).
fn reduced_step(charge: f32) -> u8 {
    let reached = (charge * 3.0).floor();
    reached.clamp(0.0, 2.0) as u8
}

/// One hold: the filament's charge, the timers that decide when the charge is
/// full or a key was let go, and the single send it authorizes.
#[derive(Clone)]
struct Hold {
    state: AppState,
    kind: ConsentKind,
    language: Language,
    /// The card's content total, as the status a completed hold leaves
    /// reports it (`Sent · N B`).
    total: Rc<Cell<u64>>,
    /// The manifest the charge belongs to: a scope switch replaces it, and
    /// [`Hold::rebuild`] starts the hold over for the new bytes.
    manifest: Rc<RefCell<Option<OutboundConsent>>>,
    /// Whether the hold control has the keyboard focus: only there do `Enter`
    /// and `Space` charge it.
    focused: Rc<Cell<bool>>,
    /// What the control's label reads.
    label: Signal<HoldLabel>,
    /// The charge, 0..1, as the filament draws it.
    heat: AnimatedValue,
    /// The charge in progress: when it began on the engine's clock. The
    /// charge itself is always the same: a press that follows a release
    /// starts from zero, and nothing resumes it.
    charging: Rc<Cell<Option<Instant>>>,
    /// Where an early release left the charge, and when: the decay reads it.
    released: Rc<Cell<Option<(f32, Instant)>>>,
    /// The input that owns the charge in progress: the pointer, `Ctrl+Enter`,
    /// or `Enter`/`Space` on the focused Send control. A press through
    /// another path (or a fresh press on the same one) cancels it.
    path: Rc<Cell<Option<HoldPath>>>,
    /// The timer that completes the charge in progress.
    completion: Rc<RefCell<Option<Spawned<()>>>>,
    /// The reduced-motion steps of a charge in progress.
    steps: Rc<RefCell<Vec<Spawned<()>>>>,
    /// The timer that ends the 'Nothing was sent.' notice.
    notice: Rc<RefCell<Option<Spawned<()>>>>,
    /// The send fired: this consent is spent.
    spent: Rc<Cell<bool>>,
}

impl Hold {
    fn new(state: AppState, kind: ConsentKind, language: Language) -> Self {
        Self {
            state,
            kind,
            language,
            total: Rc::new(Cell::new(0)),
            manifest: Rc::new(RefCell::new(None)),
            focused: Rc::new(Cell::new(false)),
            label: signal(HoldLabel::Idle),
            heat: animated_value(0.0),
            charging: Rc::new(Cell::new(None)),
            released: Rc::new(Cell::new(None)),
            path: Rc::new(Cell::new(None)),
            completion: Rc::new(RefCell::new(None)),
            steps: Rc::new(RefCell::new(Vec::new())),
            notice: Rc::new(RefCell::new(None)),
            spent: Rc::new(Cell::new(false)),
        }
    }

    /// Follow the body under the card. Every rebuild makes new controls, so
    /// the old ones' state goes with them: the focus flag (a view that is
    /// removed hears no blur) and any press or key charge in flight (its
    /// release can never arrive). A *different* manifest (a scope switch) also
    /// re-arms the hold, because the charge belongs to the binding that just
    /// went away.
    fn rebuild(&self, manifest: Option<&OutboundConsent>, total: u64) {
        let changed = {
            let stored = self.manifest.borrow();
            let stored: &Option<OutboundConsent> = &stored;
            stored.as_ref() != manifest
        };
        self.total.set(total);
        self.cancel_completion();
        self.cancel_steps();
        self.charging.set(None);
        self.released.set(None);
        self.heat.stop();
        self.focused.set(false);
        self.path.set(None);
        if self.spent.get() {
            // The send already left: its label and full charge stay until the
            // card unmounts.
            return;
        }
        self.heat.set(0.0);
        self.end_notice();
        if !changed {
            // The same binding, a new body: the charge it may have started is
            // gone with the old controls, and the card says so again.
            self.label.set(HoldLabel::Idle);
            return;
        }
        *self.manifest.borrow_mut() = manifest.cloned();
        self.spent.set(false);
        self.label.set(HoldLabel::Idle);
    }

    /// A pointer press starts a hold from zero: one press, one charge, and
    /// any charge another path owns ends now.
    fn press(&self) {
        if self.spent.get() {
            return;
        }
        self.end_notice();
        self.begin(HoldPath::Pointer);
    }

    /// Starts a charge from zero for `path` and schedules the completion a
    /// full [`motion::FILAMENT_CHARGE_MS`] later. A charge already in flight
    /// ends with this call: a fresh press always starts over.
    fn begin(&self, path: HoldPath) {
        self.end_notice();
        self.cancel_completion();
        self.cancel_steps();
        self.path.set(Some(path));
        self.charging.set(Some(nsh_core::time::now()));
        self.released.set(None);
        self.heat.stop();
        let hold = self.clone();
        let slot = self.completion.clone();
        if motion::reduced_motion() {
            // Reduced motion: the same charge, drawn in three discrete steps.
            self.press_reduced(0.0);
            *slot.borrow_mut() = Some(set_timeout(
                Duration::from_millis(motion::FILAMENT_CHARGE_MS as u64),
                move || {
                    hold.completion.borrow_mut().take();
                    hold.complete();
                },
            ));
            return;
        }
        self.label.set(HoldLabel::Holding);
        self.heat
            .timing_with(1.0, Timing::new(motion::FILAMENT_CHARGE_MS, motion::LINEAR));
        *slot.borrow_mut() = Some(set_timeout(
            Duration::from_millis(motion::FILAMENT_CHARGE_MS as u64),
            move || {
                hold.completion.borrow_mut().take();
                hold.complete();
            },
        ));
    }

    /// The reduced-motion charge: the filament jumps to a third and then two
    /// thirds (labelled `1/3`, `2/3`), and [`Hold::complete`] reads `Sending`.
    /// The charge itself is the same, so the hold length and every
    /// authorization are unchanged.
    fn press_reduced(&self, value: f32) {
        self.cancel_steps();
        let third = motion::FILAMENT_CHARGE_MS / 3.0;
        let elapsed = value * motion::FILAMENT_CHARGE_MS;
        for step in [1_u8, 2] {
            if step <= reduced_step(value) {
                self.step_to(step);
                continue;
            }
            let wait = f32::from(step) * third - elapsed;
            let hold = self.clone();
            let slot = self.steps.clone();
            slot.borrow_mut()
                .push(set_timeout(Duration::from_millis(wait as u64), move || {
                    hold.step_to(step)
                }));
        }
    }

    /// One reduced-motion step: the filament jumps to its third and the label
    /// says where the hold is.
    fn step_to(&self, step: u8) {
        if self.spent.get() || self.charging.get().is_none() {
            return;
        }
        self.heat.stop();
        self.heat.set(f32::from(step) / 3.0);
        self.label.set(if step == 1 {
            HoldLabel::StepOne
        } else {
            HoldLabel::StepTwo
        });
    }

    /// The press ended before the charge was full: cool back and say so. The
    /// authorization is zero, whatever the filament drew.
    fn release(&self) {
        if self.spent.get() {
            return;
        }
        self.path.set(None);
        let Some(at) = self.charging.take() else {
            return;
        };
        let value = charged(0.0, elapsed_ms(at));
        self.released.set(Some((value, nsh_core::time::now())));
        self.cancel_completion();
        self.cancel_steps();
        self.cool(value);
        self.label.set(HoldLabel::Released);
        let hold = self.clone();
        let slot = self.notice.clone();
        *slot.borrow_mut() = Some(set_timeout(
            Duration::from_millis(motion::CONSENT_NOTICE_MS),
            move || {
                hold.notice.borrow_mut().take();
                if hold.charging.get().is_none() && !hold.spent.get() {
                    hold.label.set(HoldLabel::Idle);
                }
            },
        ));
    }

    /// Drops the charge and its completion timer without a notice: the card
    /// is leaving, so nothing it said would be seen.
    fn cancel_hold(&self) {
        if self.spent.get() {
            return;
        }
        self.path.set(None);
        self.charging.set(None);
        self.released.set(None);
        self.cancel_completion();
        self.cancel_steps();
        self.end_notice();
        self.heat.stop();
        self.heat.set(0.0);
        self.label.set(HoldLabel::Idle);
    }

    /// One raw `KeyDown` on the focused card. A watched key (Enter, Space or
    /// a modifier) is the card's; an auto-repeat neither charges nor
    /// cancels. A fresh press of Enter/Space on the focused Send control
    /// begins from zero; anywhere else in the card it cancels the card, as
    /// the focused default (Cancel) does. A fresh modifier press cancels the
    /// charge and starts nothing; `Ctrl+Enter` begins the charge wherever the
    /// focus is inside the card.
    fn key_down(&self, event: &KeyEvent) {
        if self.spent.get() {
            return;
        }
        let role = role_of(&event.key);
        if role == KeyRole::Other || event.repeat {
            return;
        }
        match role {
            KeyRole::Modifier => self.release(),
            KeyRole::Enter => {
                if event.modifiers.ctrl || event.modifiers.cmd {
                    self.begin(HoldPath::CtrlEnter);
                } else if event.modifiers.shift || event.modifiers.alt {
                    // A shifted Enter is not the hold: it sits in the capture
                    // so the platform does not press the focused control.
                } else if self.focused.get() {
                    self.begin(HoldPath::SendEnter);
                } else {
                    cancel(&self.state, self.kind);
                }
            }
            KeyRole::Space => {
                if event.modifiers.ctrl
                    || event.modifiers.cmd
                    || event.modifiers.shift
                    || event.modifiers.alt
                {
                } else if self.focused.get() {
                    self.begin(HoldPath::SendSpace);
                } else {
                    cancel(&self.state, self.kind);
                }
            }
            KeyRole::Other => {}
        }
    }

    /// One raw `KeyUp` on the focused card: the release of a watched key
    /// ends the keyboard charge in progress. A pointer hold is the pointer's
    /// to end, so an unrelated release never breaks it.
    fn key_up(&self, event: &KeyEvent) {
        if self.spent.get() || role_of(&event.key) == KeyRole::Other {
            return;
        }
        if self.path.get().is_some_and(HoldPath::is_keyboard) {
            self.release();
        }
    }

    /// The charge is full: authorize exactly once. The focused card heard
    /// every transition that could have ended the hold; nothing else is
    /// queried.
    fn complete(&self) {
        if self.spent.get() {
            return;
        }
        self.spent.set(true);
        self.path.set(None);
        self.charging.set(None);
        self.released.set(None);
        self.cancel_completion();
        self.cancel_steps();
        self.end_notice();
        self.heat.stop();
        self.heat.set(1.0);
        self.label.set(if motion::reduced_motion() {
            HoldLabel::StepThree
        } else {
            HoldLabel::Sent
        });
        self.state.notify_count_at(
            crate::state::StatusSurface::Review,
            crate::state::StatusKind::Success,
            format!("{} · ", S::Sent.text(self.language)),
            sent_bytes(self.language, self.total.get()),
        );
        if self.kind == ConsentKind::Revision {
            self.state.send_revision();
        } else if self.kind == ConsentKind::Translation {
            self.state.send_translation();
        } else {
            self.state.send_review();
        }
    }

    /// The drawn cooling of a released filament: the same exponential, sampled
    /// into one keyframe track from where the release left it, and one 100ms
    /// crossfade under reduced motion.
    fn cool(&self, from: f32) {
        self.heat.stop();
        if motion::reduced_motion() {
            self.heat
                .timing(0.0, motion::REDUCED_CROSSFADE_MS, motion::EASE_IN_OUT);
            return;
        }
        let frames: Vec<Keyframe> = (0..16)
            .map(|index| {
                let u = index as f32 / 15.0;
                Keyframe::at(u, cooled(from, u * motion::FILAMENT_COOL_MS)).ease(motion::LINEAR)
            })
            .collect();
        let mut timing = KeyframeTiming::new(motion::FILAMENT_COOL_MS);
        timing.easing = motion::LINEAR;
        self.heat.keyframes(frames, timing);
    }

    fn cancel_completion(&self) {
        if let Some(task) = self.completion.borrow_mut().take() {
            task.cancel();
        }
    }

    fn cancel_steps(&self) {
        for task in self.steps.borrow_mut().drain(..) {
            task.cancel();
        }
    }

    fn end_notice(&self) {
        if let Some(task) = self.notice.borrow_mut().take() {
            task.cancel();
        }
    }
}

/// The filament's colour: the ramp's three stops, interpolated by the charge.
trait FilamentGlow {
    fn filament_glow(&self) -> AnimatedNode;
}

impl FilamentGlow for AnimatedValue {
    fn filament_glow(&self) -> AnimatedNode {
        // The ramp resolves per appearance, so the filament follows the theme
        // without the card being rebuilt.
        let ramp: Vec<AdaptiveColor> = tokens::LIGHT
            .filament
            .iter()
            .zip(tokens::DARK.filament.iter())
            .map(|(light, dark)| AdaptiveColor::new(*light, *dark))
            .collect();
        self.interpolate_colors(&[0.0, 0.5, 1.0], &ramp)
    }
}

// ─── copy ─────────────────────────────────────────────────────────────────

/// The card's own strings. A missing translation is a compile error, as in
/// `i18n.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum S {
    HeadlineReview,
    HeadlineRevision,
    HeadlineTranslation,
    To,
    Transport,
    Api,
    Proxy,
    Unencrypted,
    LocalModel,
    Remote,
    ProxySystem,
    ProxyDisabled,
    Scope,
    ScopeSelection,
    ScopeDocument,
    ScopeContext,
    LeavesThisDevice,
    StaysHere,
    MetadataOnly,
    RequestFraming,
    FramingReason,
    Boundary,
    Total,
    DocumentName,
    SelectionName,
    NamedSourceName,
    PackageFileName,
    OmissionName,
    ExcludedSourceName,
    ReviewContextName,
    AnswersName,
    DocumentReason,
    SelectionReason,
    NamedSourceReason,
    PackageFileReason,
    OmissionReason,
    ExcludedSourceReason,
    ReviewContextReason,
    AnswersReason,
    NothingWasSent,
    KeepHolding,
    StepOne,
    StepTwo,
    Sending,
    Sent,
    HoldToSend,
    OrHold,
}

impl S {
    fn text(self, language: Language) -> &'static str {
        match language {
            Language::English => match self {
                S::HeadlineReview => "Send for review?",
                S::HeadlineRevision => "Ask for a revision?",
                S::HeadlineTranslation => "Send for translation?",
                S::To => "To",
                S::Transport => "Transport",
                S::Api => "API",
                S::Proxy => "Proxy",
                S::Unencrypted => "HTTP (not encrypted)",
                S::LocalModel => "Local model",
                S::Remote => "Remote",
                S::ProxySystem => "may use the configured system proxy",
                S::ProxyDisabled => "disabled",
                S::Scope => "Scope",
                S::ScopeSelection => "Selection",
                S::ScopeDocument => "Document",
                S::ScopeContext => "Document + context",
                S::LeavesThisDevice => "Leaves this device",
                S::StaysHere => "Stays here",
                S::MetadataOnly => "Path, size and hash only",
                S::RequestFraming => "Request framing (not counted)",
                S::FramingReason => "headers and JSON beyond the content above",
                S::Boundary => "This content and the request framing leave this device.",
                S::Total => "Content total",
                S::DocumentName => "the document",
                S::SelectionName => "the selection",
                S::NamedSourceName => "named source",
                S::PackageFileName => "package file",
                S::OmissionName => "not sent",
                S::ExcludedSourceName => "named source",
                S::ReviewContextName => "Review context",
                S::AnswersName => "Answers",
                S::DocumentReason => "the document",
                S::SelectionReason => "selected text only",
                S::NamedSourceReason => "named source",
                S::PackageFileReason => "in the package",
                S::OmissionReason => "not sent with the package",
                S::ExcludedSourceReason => "not included",
                S::ReviewContextReason => "the review it builds on",
                S::AnswersReason => "your answers",
                S::NothingWasSent => "Nothing was sent.",
                S::KeepHolding => "Keep holding…",
                S::StepOne => "1/3",
                S::StepTwo => "2/3",
                S::Sending => "Sending",
                S::Sent => "Sent",
                S::HoldToSend => "Hold to send",
                S::OrHold => "or hold",
            },
            Language::Chinese => match self {
                S::HeadlineReview => "发送以进行审查？",
                S::HeadlineRevision => "请求修订？",
                S::HeadlineTranslation => "发送以进行翻译？",
                S::To => "发送至",
                S::Transport => "传输",
                S::Api => "API",
                S::Proxy => "代理",
                S::Unencrypted => "HTTP（未加密）",
                S::LocalModel => "本地模型",
                S::Remote => "远程",
                S::ProxySystem => "可能使用已配置的系统代理",
                S::ProxyDisabled => "已禁用",
                S::Scope => "范围",
                S::ScopeSelection => "选区",
                S::ScopeDocument => "文档",
                S::ScopeContext => "文档 + 上下文",
                S::LeavesThisDevice => "离开此设备",
                S::StaysHere => "留在本机",
                S::MetadataOnly => "仅路径、大小与哈希",
                S::RequestFraming => "请求封装（未计入合计）",
                S::FramingReason => "上述内容之外的标头与 JSON",
                S::Boundary => "此内容与请求封装都会离开此设备。",
                S::Total => "内容合计",
                S::DocumentName => "文档",
                S::SelectionName => "选区",
                S::NamedSourceName => "指定来源",
                S::PackageFileName => "包内文件",
                S::OmissionName => "未发送",
                S::ExcludedSourceName => "指定来源",
                S::ReviewContextName => "审查上下文",
                S::AnswersName => "回答",
                S::DocumentReason => "文档本身",
                S::SelectionReason => "仅选中的文本",
                S::NamedSourceReason => "指定来源",
                S::PackageFileReason => "包内文件",
                S::OmissionReason => "未随包发送",
                S::ExcludedSourceReason => "未包含",
                S::ReviewContextReason => "所依据的审查",
                S::AnswersReason => "你的回答",
                S::NothingWasSent => "未发送任何内容。",
                S::KeepHolding => "继续按住…",
                S::StepOne => "1/3",
                S::StepTwo => "2/3",
                S::Sending => "发送中",
                S::Sent => "已发送",
                S::HoldToSend => "按住发送",
                S::OrHold => "或按住",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_core::model::{ModelOperation, ModelRequestDisclosure, OutboundScope, Provider};
    use nsh_core::keymap::Modifiers;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone, Debug, PartialEq, Eq)]
    pub(super) struct FocusTrace(Vec<FocusStop>);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum FocusStop {
        Card,
        Scope,
        Cancel,
        Hold,
    }

    pub(super) fn record_focus(stop: FocusStop) {
        global::<FocusTrace>().update(|trace| {
            if trace.0.len() == 3 {
                trace.0.remove(0);
            }
            trace.0.push(stop);
        });
    }

    fn disclosure(scope: OutboundScope) -> ModelRequestDisclosure {
        ModelRequestDisclosure::new(
            ModelOperation::Review,
            EndpointIdentity::parse(Provider::OpenAiResponses, None).expect("endpoint"),
            scope,
        )
    }

    fn item(origin: OutboundOrigin, path: &str, bytes: u64, content_leaves: bool) -> OutboundItem {
        OutboundItem {
            origin,
            path: Some(path.to_owned()),
            reason: None,
            bytes,
            content_leaves,
        }
    }

    #[test]
    fn the_total_is_the_sum_of_the_rows_on_screen() {
        // A document with one named source: the total is their content, and
        // the request's own framing is disclosed without a figure.
        let consent = OutboundConsent {
            disclosure: disclosure(OutboundScope::document(600)),
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 600, true),
                item(OutboundOrigin::NamedSource, "~/AGENTS.md", 200, true),
            ],
            stays: Vec::new(),
        };
        let rows = rows(&consent, Language::English);
        assert_eq!(rows.total, 800);
        assert_eq!(
            rows.leaving.iter().map(|row| row.bytes).sum::<u64>(),
            rows.total,
            "the printed total is the sum of the rows above it"
        );
    }

    #[test]
    fn a_metadata_only_entry_is_listed_but_never_counted() {
        // A skill package with a binary asset: its raw bytes stay here; its
        // path, size and hash leave as metadata, beyond the total.
        let consent = OutboundConsent {
            disclosure: disclosure(OutboundScope::document(2_140)),
            items: vec![
                item(OutboundOrigin::PackageFile, "SKILL.md", 2_140, true),
                item(OutboundOrigin::PackageFile, "scripts/run.py", 1_388, true),
                item(
                    OutboundOrigin::PackageFile,
                    "assets/logo.png",
                    12_004,
                    false,
                ),
            ],
            stays: vec![item(OutboundOrigin::Omission, "notes/old.md", 0, false)],
        };
        let rows = rows(&consent, Language::English);
        assert_eq!(rows.total, 3_528, "the package's content");
        assert_eq!(
            rows.leaving.iter().map(|row| row.bytes).sum::<u64>(),
            rows.total,
            "the total is the rows above it"
        );
        assert_eq!(rows.metadata.len(), 1, "the binary is its own group");
        assert_eq!(rows.metadata[0].bytes, 12_004, "its disclosed size stays");
        assert!(
            !rows.leaving.iter().any(|row| row.label.contains("logo")),
            "its size is not part of the total"
        );
        assert_eq!(rows.stays.len(), 1);
    }

    #[test]
    fn a_revision_lists_only_its_items() {
        let consent = OutboundConsent {
            disclosure: ModelRequestDisclosure::new(
                ModelOperation::Revision,
                EndpointIdentity::parse(Provider::OpenAiResponses, None).expect("endpoint"),
                OutboundScope::document(400),
            ),
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 400, true),
                item(OutboundOrigin::ReviewContext, "Review context", 900, true),
                item(OutboundOrigin::Answers, "Answers", 120, true),
            ],
            stays: Vec::new(),
        };
        let rows = rows(&consent, Language::English);
        assert_eq!(rows.total, 1_420);
    }

    #[test]
    fn a_manifest_taller_than_the_card_scrolls_and_hides_no_file() {
        // An Agent Skill package with more files in every group than the
        // card's height holds: each file is still on the card, by path, size
        // and reason, and the hold sends exactly the total of those rows.
        let consent = large_consent();
        let budget = (rows_max_h(800.0 * CARD_MAX_FRACTION) / ROW_H) as usize;
        assert!(
            consent.items.len() + consent.stays.len() > budget,
            "the manifest overflows the {budget}-row region"
        );
        let mut app = start_harness();
        app.press("id:harness-large")
            .expect("open the large manifest");
        app.wait_for("text:skills/file39.md")
            .expect("the last file that leaves");
        for item in consent.items.iter().chain(&consent.stays) {
            let path = item.path.as_deref().expect("a path");
            assert!(app.shows(path), "{path} is on the card");
        }
        assert!(app.shows("4,000 B"), "the total is every leaving row");
        assert!(app.shows("Hold to send"), "the hold stays on the card");
        assert!(
            app.wait_for("id:consent-rows-scroll").is_ok(),
            "the rows scroll in place"
        );
    }

    #[test]
    fn a_tap_can_never_complete_a_hold() {
        // A tap is orders shorter than the charge a send needs: whatever it
        // leaves, it cannot reach the completion.
        let tapped = charged(0.0, 50.0);
        assert!(
            remaining_ms(tapped) > 0.0,
            "a single tap leaves charge to hold for"
        );
    }

    #[test]
    fn a_release_leaves_no_charge_a_new_press_could_resume() {
        // One continuous hold authorizes. The charged value a release leaves
        // is drawing only: `Hold::begin` always starts the next press at zero,
        // so two presses add up to the longer one alone, never a carry-over.
        let released = charged(0.0, 500.0);
        assert!(
            remaining_ms(released) > 0.0 && remaining_ms(released) < motion::FILAMENT_CHARGE_MS,
            "half a hold is a real, unfinished charge"
        );
        assert_eq!(
            remaining_ms(0.0),
            motion::FILAMENT_CHARGE_MS,
            "a new press always starts the full hold"
        );
    }

    #[test]
    fn the_bytes_column_reads_like_the_spec() {
        assert_eq!(bytes_label(640), "640 B");
        assert_eq!(bytes_label(2_140), "2,140 B");
        assert_eq!(bytes_label(5_072), "5,072 B");
        assert_eq!(bytes_label(0), "0 B");
    }

    #[test]
    fn a_loopback_endpoint_is_labelled_unencrypted() {
        let endpoint = EndpointIdentity::parse(
            Provider::OpenAiResponses,
            Some("http://127.0.0.1:8080/custom/v1/"),
        )
        .expect("loopback endpoint");
        let terms = destination(Language::English, &endpoint)
            .map(|(term, value)| (term.text(Language::English), value));
        assert_eq!(
            terms,
            [
                ("To", "Local model · 127.0.0.1:8080".to_owned()),
                ("Transport", "HTTP (not encrypted)".to_owned()),
                ("API", "OpenAI Responses /custom/v1".to_owned()),
                ("Proxy", "disabled".to_owned()),
            ],
            "every fact, in the order the line had them"
        );
    }

    #[test]
    fn a_remote_endpoint_discloses_the_location_and_the_proxy() {
        let endpoint =
            EndpointIdentity::parse(Provider::OpenAiResponses, None).expect("the default endpoint");
        let [to, transport, _, proxy] = destination(Language::English, &endpoint);
        assert!(to.1.starts_with("Remote · "), "{to:?}");
        assert_eq!(transport.1, "HTTPS");
        assert_eq!(proxy.1, "may use the configured system proxy");
    }

    /// A manifest of the shape the state builds: one document and one named
    /// source.
    fn sample_consent() -> OutboundConsent {
        OutboundConsent {
            disclosure: disclosure(
                OutboundScope::document_with_effective_agent_context(600, ["~/AGENTS.md"])
                    .expect("a named source"),
            ),
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 600, true),
                item(OutboundOrigin::NamedSource, "~/AGENTS.md", 200, true),
            ],
            stays: Vec::new(),
        }
    }

    /// A manifest of the shape a switch *to* the plain document leaves: the
    /// context gone, its source staying.
    fn sample_consent_without_context() -> OutboundConsent {
        OutboundConsent {
            disclosure: disclosure(OutboundScope::document(600)),
            items: vec![item(OutboundOrigin::Document, "plan.md", 600, true)],
            stays: vec![item(
                OutboundOrigin::ExcludedSource,
                "~/AGENTS.md",
                200,
                false,
            )],
        }
    }

    /// An Agent Skill package taller than the card: 40 files leave, 12 leave
    /// as metadata and 12 stay.
    fn large_consent() -> OutboundConsent {
        let file =
            |name: String, bytes, leaves| item(OutboundOrigin::PackageFile, &name, bytes, leaves);
        OutboundConsent {
            disclosure: disclosure(OutboundScope::document(1)),
            items: (0..40)
                .map(|index| file(format!("skills/file{index}.md"), 100, true))
                .chain((0..12).map(|index| file(format!("skills/asset{index}.png"), 4_000, false)))
                .collect(),
            stays: (0..12)
                .map(|index| {
                    item(
                        OutboundOrigin::ExcludedSource,
                        &format!("stay{index}.md"),
                        50,
                        false,
                    )
                })
                .collect(),
        }
    }

    /// The card over a state that already has a consent waiting: the shell's
    /// `when(is_open)` around [`card`]. One control stands in for the scope
    /// switch, replacing the manifest the card is bound to (the card's own
    /// switch rebinds a prepared Review, which this harness does not have).
    #[component]
    fn Harness() -> impl IntoElement {
        let focus_trace = global::<FocusTrace>();
        let state = AppState::new();
        state
            .review_consent
            .set(Some(SharedString::from("disclosure text")));
        state.review_manifest.set(Some(sample_consent()));
        let card_state = state.clone();
        let switch_state = state.clone();
        let reopen_state = state.clone();
        let large_state = state.clone();
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(
                // A plain pressable view: its test id is the one the harness
                // presses, and it stays out of Tab's stops.
                view()
                    .test_id("harness-swap")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        switch_state
                            .review_manifest
                            .set(Some(sample_consent_without_context()));
                    })
                    .child(prims::rest("swap")),
            )
            .child(
                view()
                    .test_id("harness-large")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        large_state.review_manifest.set(Some(large_consent()));
                    })
                    .child(prims::rest("large")),
            )
            .child(
                view()
                    .test_id("harness-reopen")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        reopen_state
                            .review_consent
                            .set(Some(SharedString::from("disclosure text")));
                        reopen_state.review_manifest.set(Some(sample_consent()));
                    })
                    .child(prims::rest("reopen")),
            )
            .child(when(
                move || is_open(&state),
                move || card(card_state.clone(), Language::English),
            ))
            .child(
                view()
                    .absolute()
                    .w(px(1.0))
                    .h(px(1.0))
                    .opacity(0.0)
                    .child(text(move || {
                        SharedString::from(format!("focus order: {:?}", focus_trace.get().0))
                    })),
            )
    }

    #[component]
    fn ReducedMotionHarness() -> impl IntoElement {
        let state = AppState::new();
        let open_state = state.clone();
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(prims::quiet_action("reduced-open", "Open", move || {
                open_state
                    .review_consent
                    .set(Some(SharedString::from("disclosure text")));
                open_state.review_manifest.set(Some(sample_consent()));
            }))
            .child(crate::ui::shell::overlay_layer(state, Language::English))
    }

    #[component]
    fn SentFadeHarness() -> impl IntoElement {
        let state = AppState::new();
        let open_state = state.clone();
        let clear_state = state.clone();
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(prims::quiet_action("sent-fade-open", "Open", move || {
                open_state
                    .review_consent
                    .set(Some(SharedString::from("disclosure text")));
                open_state.review_manifest.set(Some(sample_consent()));
            }))
            .child(
                view()
                    .test_id("sent-fade-clear")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        clear_state.review_consent.set(None);
                        clear_state.review_manifest.set(None);
                    })
                    .child(prims::rest("clear")),
            )
            .child(crate::ui::shell::overlay_layer(state, Language::English))
    }

    pub(super) fn start_harness() -> Headless {
        // A late frame once ran hold completion before its timer; manual time orders them.
        let mut config = EngineConfig::new(Root::of::<Harness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        // A platform that focuses ordinary views and delivers focused-view
        // keys, as the Windows host does: the card takes the focus and its
        // raw handlers hear the tests' key events.
        config.environment.os = nsh_core::element::Os::Linux;
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:consent-card").expect("the card");
        app
    }

    fn press_without_settling(app: &Headless, selector: &str) {
        use std::sync::mpsc;

        let id = app.screen().id(selector).expect("the control");
        let engine = app.engine().control();
        engine.dispatch_event(
            id,
            nsh_core::EventKind::Press,
            nsh_core::EventPayload::Press(Default::default()),
        );
        let (sent, received) = mpsc::channel();
        engine.flush(move |_| {
            let _ = sent.send(());
        });
        received
            .recv_timeout(Duration::from_secs(5))
            .expect("the press flushed");
    }

    /// The production control carries no long-press delay: the harness holds
    /// past the charge itself.
    fn hold_send(app: &mut Headless) {
        use nsh_core::geometry::point;

        app.drag_and_hold("id:consent-send", point(px(0.0), px(0.0)))
            .expect("hold the send control");
        app.run_for(Duration::from_millis(1_100))
            .expect("the full charge");
        app.drop_held().expect("release the send control");
    }

    #[test]
    fn reduced_motion_keeps_transforms_out_of_the_card_subtree() {
        let mut config = EngineConfig::new(Root::of::<ReducedMotionHarness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("the environment");
        let binding_events = Arc::new(Mutex::new(Vec::<(
            nsh_core::NodeId,
            Vec<nsh_core::animation::AnimationProp>,
        )>::new()));
        app.observe({
            let binding_events = binding_events.clone();
            move |_, batch| {
                for item in &batch.items {
                    if let nsh_core::MountItem::AnimationBindings { id, bindings } = item {
                        binding_events.lock().expect("lock the test record").push((
                            *id,
                            bindings.iter().map(|(property, _)| *property).collect(),
                        ));
                    }
                }
            }
        });
        app.press("id:reduced-open").expect("open consent");
        app.wait_for("id:consent-card").expect("the consent card");
        let motion_card = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("consent-motion-card"))
            .expect("the consent motion card")
            .id;
        use nsh_core::geometry::point;
        app.drag_and_hold("id:consent-send", point(px(0.0), px(0.0)))
            .expect("start the hold");
        app.drop_held().expect("release the hold");
        app.settle();

        let layout_nodes = app.screen().layout_nodes();
        let card_nodes: Vec<_> = layout_nodes
            .iter()
            .filter(|node| {
                let mut current = node.id;
                loop {
                    if current == motion_card {
                        return true;
                    }
                    let Some(parent) = layout_nodes
                        .iter()
                        .find(|candidate| candidate.id == current)
                        .map(|candidate| candidate.parent)
                    else {
                        return false;
                    };
                    if parent == current {
                        return false;
                    }
                    current = parent;
                }
            })
            .map(|node| node.id)
            .collect();
        let binding_events = binding_events.lock().expect("lock the test record");
        let card_bindings: Vec<_> = binding_events
            .iter()
            .filter(|(id, _)| card_nodes.contains(id))
            .collect();
        assert!(
            card_bindings
                .iter()
                .any(|(_, properties)| !properties.is_empty()),
            "the card subtree has recorded animation bindings"
        );
        assert!(
            card_bindings.iter().all(|(_, properties)| {
                properties.iter().all(|property| {
                    !matches!(property, nsh_core::animation::AnimationProp::Transform(_))
                })
            }),
            "no animated binding in the reduced-motion card subtree uses a transform"
        );
    }

    #[test]
    fn the_card_shows_the_manifest_and_a_tap_authorizes_nothing() {
        let mut app = start_harness();
        app.wait_for("id:consent-total").expect("the total");
        assert!(app.shows("Send for review?"), "the headline");
        assert!(app.shows("Leaves this device"), "the manifest caption");
        assert!(app.shows("plan.md"), "the document row");
        assert!(app.shows("~/AGENTS.md"), "the named source row");
        assert!(app.shows("800 B"), "the content total");
        assert!(app.shows("Hold to send"), "the hold's label");
        assert!(app.shows("Content total"), "the sum named for what it is");
        assert!(
            app.shows("Request framing (not counted)"),
            "the framing disclosure, without a figure"
        );
        assert!(
            !app.shows("Protocol framing"),
            "no figure is claimed for the wire request"
        );
        assert!(
            app.wait_for("id:consent-key-hint >> text:Enter").is_ok(),
            "the card's hint names the hold's keys"
        );
        assert!(
            app.wait_for("id:consent-destination >> text:/^Remote · /")
                .is_ok()
                && app.shows("may use the configured system proxy"),
            "the destination discloses the location and the proxy"
        );
        // The scope row: the current scope in ink, the other one switchable.
        assert!(app.shows("Document + context"), "the scope in force");
        assert!(app.shows("Document"), "the scope it can switch back to");
        assert!(
            app.wait_for("id:consent-scope-document").is_ok(),
            "the switch back to the plain document is on the card"
        );
        // A tap on the send control authorizes nothing.
        app.press("id:consent-send").expect("tap the send control");
        app.wait_for("text:Nothing was sent.")
            .expect("the early release says so");
        assert!(!app.shows("Sent"), "nothing left the device");
    }

    /// A Revision of a reviewed selection, waiting on the card.
    #[component]
    fn RevisionSelectionHarness() -> impl IntoElement {
        let state = AppState::new();
        state
            .revision_consent
            .set(Some(SharedString::from("disclosure text")));
        state.revision_manifest.set(Some(OutboundConsent {
            disclosure: ModelRequestDisclosure::new(
                ModelOperation::Revision,
                EndpointIdentity::parse(Provider::OpenAiResponses, None).expect("endpoint"),
                OutboundScope::selection(120),
            ),
            items: vec![item(OutboundOrigin::Selection, "plan.md", 120, true)],
            stays: Vec::new(),
        }));
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(card(state, Language::English))
    }

    #[test]
    fn a_revision_of_a_selection_names_its_scope_and_offers_no_other() {
        let mut config = EngineConfig::new(Root::of::<RevisionSelectionHarness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:consent-scope-selection >> text:Selection")
            .expect("the scope in force, in words");
        assert!(
            app.shows("Ask for a revision?"),
            "the revision card's question"
        );
        let scope_ids: Vec<String> = app
            .screen()
            .layout_nodes()
            .iter()
            .filter_map(|node| node.test_id.as_deref().map(str::to_owned))
            .filter(|id| id.starts_with("consent-scope-"))
            .collect();
        assert_eq!(
            scope_ids,
            ["consent-scope-selection"],
            "a Revision's scope cannot switch, so no other scope is drawn"
        );
    }

    #[component]
    fn TranslationHarness() -> impl IntoElement {
        let state = AppState::new();
        state
            .translation_consent
            .set(Some(SharedString::from("disclosure text")));
        state.translation_manifest.set(Some(OutboundConsent {
            disclosure: ModelRequestDisclosure::new(
                ModelOperation::Translation,
                EndpointIdentity::parse(Provider::OpenAiChat, None).expect("endpoint"),
                OutboundScope::document(120),
            ),
            items: vec![item(OutboundOrigin::Document, "plan.md", 120, true)],
            stays: Vec::new(),
        }));
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(card(state, Language::English))
    }

    #[test]
    fn translation_uses_the_hold_to_send_card() {
        let mut config = EngineConfig::new(Root::of::<TranslationHarness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:consent-card").expect("the card");
        assert!(
            app.shows("Send for translation?"),
            "the translation card's question: {:?}",
            app.screen().texts()
        );
        // The document scope is in force, in words, and no other scope is
        // offered: a Translation always sends the document.
        assert!(app.shows("Document"), "the scope in force");
        let scope_ids: Vec<String> = app
            .screen()
            .layout_nodes()
            .iter()
            .filter_map(|node| node.test_id.as_deref().map(str::to_owned))
            .filter(|id| id.starts_with("consent-scope-"))
            .collect();
        assert_eq!(
            scope_ids,
            ["consent-scope-document"],
            "a Translation's scope cannot switch"
        );
        assert!(!app.shows("Sent"), "nothing left the device");
        hold_send(&mut app);
        assert!(
            app.shows("Sent"),
            "the full hold authorizes the translation send"
        );
    }

    #[test]
    fn escape_cancels_and_closes_the_card() {
        let mut app = start_harness();
        app.press_key("escape").expect("escape");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the card closed");
    }

    #[test]
    fn the_cancel_control_cancels() {
        // The cancel control is one focusable view of its own (not a
        // component's boundary): its press still authorizes nothing.
        let mut app = start_harness();
        app.press("id:consent-cancel-button").expect("press Cancel");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the card closed");
    }

    #[test]
    fn a_held_press_sends_once() {
        let mut app = start_harness();
        hold_send(&mut app);
        assert!(app.shows("Sent"), "a full hold authorized one send");
    }

    #[test]
    fn the_consent_charge_stays_full_through_the_sent_fade() {
        use nsh_core::{
            MountItem,
            animation::{AnimationProp, Animator},
        };

        let mut config = EngineConfig::new(Root::of::<SentFadeHarness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:sent-fade-open").expect("the harness");
        let batches = Arc::new(Mutex::new(Vec::<MountItem>::new()));
        app.observe({
            let batches = batches.clone();
            move |_, batch| {
                batches
                    .lock()
                    .expect("lock the batch record")
                    .extend(batch.items.iter().cloned());
            }
        });

        app.press("id:sent-fade-open").expect("open the consent");
        app.wait_for("id:consent-card").expect("the card");
        hold_send(&mut app);
        assert!(app.shows("Sent"), "the hold authorized the send");

        let filament = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("consent-filament"))
            .expect("the charge bar")
            .id;
        let heat = {
            let batches = batches.lock().expect("lock the batch record");
            batches
                .iter()
                .find_map(|item| match item {
                    MountItem::AnimationBindings { id, bindings } if *id == filament => {
                        bindings.iter().find_map(|(property, node)| {
                            matches!(
                                property,
                                AnimationProp::Transform(_) | AnimationProp::Opacity
                            )
                            .then_some(*node)
                        })
                    }
                    _ => None,
                })
                .expect("the charge bar's charge binding")
        };

        press_without_settling(&app, "id:sent-fade-clear");
        app.catch_up();
        assert_eq!(
            app.screen().matches("id:consent-motion-card"),
            Ok(true),
            "the card is still up for its exit fade"
        );
        assert!(app.shows("Sent"), "the fade still reads sent");

        let mut animator = Animator::default();
        {
            let batches = batches.lock().expect("lock the batch record");
            for item in batches.iter() {
                animator.apply(item);
            }
        }
        assert_eq!(
            animator.value(heat),
            1.0,
            "the charge bar stays full until the card unmounts"
        );
    }

    #[test]
    fn reduced_motion_steps_the_hold_and_still_sends() {
        // Under reduced motion the charge is the same hold with the same
        // completion and the same send; only its drawing steps. The steps
        // themselves are [`reduced_step`]'s unit test, the pointer hold here
        // cannot be sampled mid-press.
        let mut app = start_harness();
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("the environment");
        hold_send(&mut app);
        // The completion is the third step under reduced motion, so the label
        // the hold leaves is `Sending` (the ramp's would read `Sent`); only
        // [`Hold::complete`] sets either, after the full hold.
        assert!(
            app.wait_for("text:Sending").is_ok(),
            "a full hold authorized one send"
        );
    }

    #[test]
    fn reduced_motion_changes_during_a_hold_keep_progress_and_focus() {
        let mut app = start_harness();
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduced motion");
        app.set_environment("a platform that focuses views", |env| {
            env.os = nsh_core::element::Os::Linux;
        })
        .expect("the environment");
        for _ in 0..3 {
            app.press_key("tab").expect("tab");
        }
        assert!(
            app.shows("focus order: [Scope, Cancel, Hold]"),
            "the three Tab stops were {:?}",
            app.screen().texts()
        );
        dispatch_key(&mut app, " ", true, Modifiers::default(), false);
        app.wait(Duration::from_millis(350))
            .expect("the first third of the hold");
        assert!(app.shows("1/3"), "the hold reached its first third");

        app.set_environment("motion enabled", |env| env.reduce_motion = false)
            .expect("enable motion");
        let progress_kept = app.shows("1/3");
        dispatch_key(&mut app, " ", true, Modifiers::default(), true);
        let focus_kept = app.shows("1/3");
        assert!(
            progress_kept && focus_kept,
            "progress kept: {progress_kept}; hold kept focus: {focus_kept}"
        );
        key_up(&mut app, " ", Modifiers::default());
        assert!(!app.shows("Sent"), "the released hold sent nothing");
    }

    #[test]
    fn a_theme_switch_keeps_consent_hold_progress_and_focus() {
        use nsh_core::platform::ColorScheme;

        let mut app = start_harness();
        app.set_environment("a platform that focuses views", |env| {
            env.os = nsh_core::element::Os::Linux;
        })
        .expect("the environment");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduced motion");
        app.set_environment("light appearance", |env| {
            env.color_scheme = ColorScheme::Light;
        })
        .expect("light appearance");

        for _ in 0..3 {
            app.press_key("tab").expect("tab");
        }
        dispatch_key(&mut app, " ", true, Modifiers::default(), false);
        app.wait(Duration::from_millis(350))
            .expect("the first third of the hold");
        let progress_before = app.shows("1/3");

        app.set_environment("system appearance becomes dark", |env| {
            env.color_scheme = ColorScheme::Dark;
        })
        .expect("dark appearance");
        let progress_kept = app.shows("1/3");
        dispatch_key(&mut app, " ", true, Modifiers::default(), true);
        let hold_responds = app.shows("1/3");

        assert!(
            progress_before && progress_kept && hold_responds,
            "charged before theme change: {progress_before}; charge kept: {progress_kept}; hold responds: {hold_responds}"
        );
        key_up(&mut app, " ", Modifiers::default());
        assert!(!app.shows("Sent"), "the released hold sent nothing");
    }

    #[test]
    fn the_reduced_motion_steps_are_thirds_of_the_charge() {
        assert_eq!(reduced_step(0.0), 0, "nothing yet");
        assert_eq!(reduced_step(0.32), 0, "just short of the first third");
        assert_eq!(reduced_step(1.0 / 3.0), 1, "the first third");
        assert_eq!(reduced_step(0.65), 1, "just short of the second");
        assert_eq!(reduced_step(2.0 / 3.0), 2, "the second third");
        assert_eq!(reduced_step(1.0), 2, "the third step is the send itself");
    }

    #[test]
    fn a_keyboard_tap_alone_sends_nothing_and_cancels() {
        let mut app = start_harness();
        // The hold is not focused: Enter and Space take the safe default.
        app.press_key("space").expect("space");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the card closed, cancelled");
        assert!(!app.shows("Sent"), "a tap authorizes nothing");
    }

    #[test]
    fn two_enter_taps_send_nothing_and_cancel() {
        // The blocker: two accidental presses must not carry a charge over
        // the line — there is no charge at all while the hold control is not
        // focused.
        let mut app = start_harness();
        app.press_key("enter").expect("enter");
        app.press_key("enter").expect("enter again");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the first press cancelled the card");
        assert!(!app.shows("Sent"), "nothing was sent");
    }

    fn assert_mono_consent_bytes(selector: &str) {
        let mut app = start_harness();
        app.wait_for("id:consent-total").expect("consent total");
        let style = app.screen().span_style(selector).expect(selector);
        assert_eq!(
            style.font_family.expect(selector).name().as_ref(),
            tokens::FONT_SOURCE,
            "{selector}"
        );
    }

    #[test]
    fn consent_total_bytes_use_mono_figures() {
        assert_mono_consent_bytes("id:consent-total >> text:800 B");
    }

    #[test]
    fn consent_item_bytes_use_mono_figures() {
        assert_mono_consent_bytes("id:consent-row-source >> text:200 B");
    }

    #[component]
    fn BackgroundFocusHarness() -> impl IntoElement {
        let state = AppState::new();
        let open_state = state.clone();
        let card_state = state.clone();
        let presses = signal(0);
        let background_focused = signal(false);
        let background = element_ref();
        effect(move || on_key_with("f6", KeyOptions::default(), move |_| background.focus()));
        let capture_state = state.clone();
        view()
            // The shell captures Enter/Space while a consent card is open, so
            // a stray key never presses the dimmed room.
            .capture_keys(move || {
                if is_open(&capture_state) {
                    key_capture()
                } else {
                    KeyCapture::None
                }
            })
            .child(
                view()
                    .test_id("open-consent")
                    .element_ref(&background)
                    .on_focus(move || background_focused.set(true))
                    .on_blur(move || background_focused.set(false))
                    .on_press(move |_: &PressEvent| {
                        presses.update(|count| *count += 1);
                        open_state
                            .review_consent
                            .set(Some(SharedString::from("disclosure text")));
                        open_state.review_manifest.set(Some(sample_consent()));
                    })
                    .child(prims::rest("Open consent")),
            )
            .child(text(move || {
                format!("Background presses: {}", presses.get())
            }))
            .child(when(
                move || background_focused.get(),
                || text("Background focused"),
            ))
            .child(when(
                move || is_open(&state),
                move || card(card_state.clone(), Language::English),
            ))
    }

    fn shifted_key_keeps_consent_open_without_pressing_the_background(key: &str) {
        let mut config = EngineConfig::new(Root::of::<BackgroundFocusHarness>());
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
        let mut app = Headless::start(config).expect("start the harness");
        app.wait_for("id:open-consent").expect("background control");
        app.press("id:open-consent").expect("open consent");
        app.wait_for("id:consent-card").expect("the card");
        app.press_key("f6").expect("focus the background control");
        assert!(app.shows("Background focused"), "background has focus");
        assert!(app.shows("Background presses: 1"));

        app.press_key(key).expect("press the shifted key");
        assert!(
            app.shows("Background presses: 1"),
            "{key} pressed the background"
        );
        assert_eq!(
            app.screen().matches("id:consent-card"),
            Ok(true),
            "{key} closed the card"
        );
        assert!(!app.shows("Sent"), "nothing was sent");
    }

    #[test]
    fn shift_enter_is_consumed_with_background_focus() {
        shifted_key_keeps_consent_open_without_pressing_the_background("shift+enter");
    }

    #[test]
    fn shift_space_is_consumed_with_background_focus() {
        shifted_key_keeps_consent_open_without_pressing_the_background("shift+space");
    }

    #[test]
    fn shifted_keys_do_not_charge_the_focused_hold_control() {
        let mut app = start_harness();
        app.set_environment("a platform that focuses views", |env| {
            env.os = nsh_core::element::Os::Linux;
        })
        .expect("the environment");
        for _ in 0..3 {
            app.press_key("tab").expect("tab to the hold control");
        }

        for key in ["shift+enter", "shift+space"] {
            app.press_key(key).expect("press the shifted key");
            assert!(app.shows("Hold to send"), "{key} changed the hold");
            assert!(!app.shows("Nothing was sent."), "{key} started charging");
        }

        app.press_key("space").expect("charge with bare Space");
        app.wait_for("text:Nothing was sent.")
            .expect("the focused control charged and cooled");
    }

    #[test]
    fn the_card_follows_a_scope_switch() {
        // Switching scope replaces the manifest under the card: the rows, the
        // total and the hold's bytes must follow it, never lag behind.
        let mut app = start_harness();
        app.wait_for("id:consent-total").expect("the total");
        assert!(app.shows("~/AGENTS.md"), "the context source leaves");
        assert!(app.shows("800 B"), "the content with the context");
        // A tap leaves the hold's notice behind: the switch must re-arm the
        // hold, not just redraw the rows.
        app.press("id:consent-send").expect("tap the send control");
        app.wait_for("text:Nothing was sent.")
            .expect("the early release says so");
        app.press("id:harness-swap").expect("switch the scope");
        app.wait_for("id:consent-scope")
            .expect("the card stayed up");
        assert!(
            app.wait_for_gone("id:consent-row-source", Duration::from_secs(2))
                .is_ok(),
            "the source row went"
        );
        assert!(app.shows("Stays here"), "the source is shown staying");
        assert!(
            app.wait_for("text:600 B").is_ok(),
            "the total followed the manifest"
        );
        assert!(
            app.wait_for_gone("text:800 B", Duration::from_secs(2))
                .is_ok(),
            "the old total is gone"
        );
        assert!(
            app.wait_for_gone("text:Nothing was sent.", Duration::from_secs(2))
                .is_ok(),
            "the hold was re-armed, not left spent"
        );
        assert!(app.shows("Hold to send"), "the invitation is back");
        // The re-armed hold still sends, and sends the new binding.
        hold_send(&mut app);
        assert!(app.shows("Sent"), "the re-armed hold authorized the send");
    }

    #[test]
    fn the_keyboard_charges_only_the_focused_hold_control() {
        // On a platform that focuses views, Tab reaches the hold control and
        // only there does Space charge it; anywhere else Space cancels (the
        // tests above).
        let mut app = start_harness();
        app.set_environment("a platform that focuses views", |env| {
            env.os = nsh_core::element::Os::Linux;
        })
        .expect("the environment");
        // Tab's stops on this platform, in order: the scope switch, Cancel,
        // then the hold control.
        for _ in 0..3 {
            app.press_key("tab").expect("tab");
        }
        app.press_key("space").expect("space");
        // The harness's Space is a whole press (down and up), so the charge it
        // started cools and says so: the charge happened, nothing was sent.
        app.wait_for("text:Nothing was sent.")
            .expect("the focused control charged and cooled");
        assert!(app.shows("Send for review?"), "the card stayed up");
        assert!(!app.shows("Sent"), "a tap authorizes nothing");
    }

    #[test]
    fn ctrl_enter_charges_the_focused_card_and_an_early_release_sends_nothing() {
        // `Ctrl+Enter` anywhere in the focused card charges; the release ends
        // the charge at once, with nothing authorized.
        let mut app = start_harness();
        app.press_key("ctrl+enter").expect("ctrl+enter");
        assert!(
            app.shows("Nothing was sent."),
            "the charge started and the early release says so"
        );
        assert!(app.shows("Send for review?"), "the card stayed up");
        app.wait(Duration::from_millis(1_200))
            .expect("past the charge");
        assert!(!app.shows("Sent"), "an early release authorizes nothing");
    }

    #[test]
    fn a_rebuilt_body_drops_the_focus_that_went_with_it() {
        // A rebuild makes new controls, and the old ones' state goes with
        // them: a still-charging `focused` flag would let `Enter` charge a
        // control the user can no longer see.
        let mut app = start_harness();
        app.set_environment("a platform that focuses views", |env| {
            env.os = nsh_core::element::Os::Linux;
        })
        .expect("the environment");
        // Tab's stops, in order: the scope switch, Cancel, then the hold
        // control, which then holds the focus.
        for _ in 0..3 {
            app.press_key("tab").expect("tab");
        }
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(900.0)))
            .expect("resize, which rebuilds the body");
        // The rebuilt card's `Enter` is the default control's, not a charge.
        app.press_key("enter").expect("enter");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("Enter cancelled: the stale focus was dropped");
        assert!(!app.shows("Sent"), "nothing was sent");
    }

    /// The card over a state, with the status generation drawn so a hold's
    /// sends can be counted, and controls that reopen and rebuild it.
    #[component]
    fn KeyboardHarness() -> impl IntoElement {
        let state = AppState::new();
        state
            .review_consent
            .set(Some(SharedString::from("disclosure text")));
        state.review_manifest.set(Some(sample_consent()));
        let card_state = state.clone();
        let sends_state = state.clone();
        let reopen_state = state.clone();
        let swap_state = state.clone();
        view()
            .w(px(1_200.0))
            .h(px(800.0))
            .child(
                view()
                    .test_id("hold-reopen")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        reopen_state
                            .review_consent
                            .set(Some(SharedString::from("disclosure text")));
                        reopen_state.review_manifest.set(Some(sample_consent()));
                    })
                    .child(prims::rest("reopen")),
            )
            .child(
                view()
                    .test_id("hold-swap")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        swap_state
                            .review_manifest
                            .set(Some(sample_consent_without_context()));
                    })
                    .child(prims::rest("swap")),
            )
            .child(when(
                move || is_open(&state),
                move || card(card_state.clone(), Language::English),
            ))
            .child(
                text(move || format!("sends: {}", sends_state.status_generation.get()))
                    .test_id("hold-sends"),
            )
            .child(text(move || {
                let trace = global::<FocusTrace>();
                SharedString::from(format!("focus order: {:?}", trace.get().0))
            }))
    }

    fn start_keyboard_harness() -> Headless {
        let mut config = EngineConfig::new(Root::of::<KeyboardHarness>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        // A platform that focuses ordinary views, as the Windows host does:
        // the card takes the focus and its controls are Tab stops.
        config.environment.os = nsh_core::element::Os::Linux;
        // Reduced motion keeps the charge's drawing off the frame clock, so
        // synthetic key events settle without running the charge out.
        config.environment.reduce_motion = true;
        let mut app = Headless::start(config).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:consent-card").expect("the card");
        app.wait_until("the card's focus", Duration::from_secs(5), |screen| {
            screen.matches("text:focus order: [Card]") == Ok(true)
        })
        .expect("the card takes the keyboard focus");
        app
    }

    fn send_count(app: &Headless) -> u64 {
        let text = app
            .screen()
            .layout_nodes()
            .iter()
            .find(|node| node.test_id.as_deref() == Some("hold-sends"))
            .and_then(|node| node.text.as_ref().map(|text| text.text.text.to_string()))
            .expect("the send counter");
        text.trim_start_matches("sends: ")
            .parse()
            .expect("the send count")
    }

    /// One raw `KeyDown`/`KeyUp` on the focused card, as the platform's
    /// focused-view delivery sends it: a harness holds a key by sending the
    /// down and, later, the up.
    fn dispatch_key(app: &mut Headless, key: &str, down: bool, modifiers: Modifiers, repeat: bool) {
        let id = app.screen().id("id:consent-card").expect("the card");
        let mut event = KeyEvent::default();
        event.key = SharedString::from(key.to_owned());
        event.modifiers = modifiers;
        event.repeat = repeat;
        let engine = app.engine().control();
        engine.dispatch_event(
            id,
            if down {
                EventKind::KeyDown
            } else {
                EventKind::KeyUp
            },
            EventPayload::Key(event),
        );
        let (sent, received) = std::sync::mpsc::channel();
        engine.flush(move |_| {
            let _ = sent.send(());
        });
        received
            .recv_timeout(Duration::from_secs(5))
            .expect("the key flushed");
        app.catch_up();
    }

    fn ctrl_enter() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn cmd_enter() -> Modifiers {
        Modifiers {
            cmd: true,
            ..Modifiers::default()
        }
    }

    // One raw `Blur` on the card, as the platform sends it when the focus
    // leaves it for somewhere the card does not own.
    fn dispatch_blur(app: &mut Headless) {
        let id = app.screen().id("id:consent-card").expect("the card");
        let engine = app.engine().control();
        engine.dispatch_event(id, EventKind::Blur, EventPayload::None);
        let (sent, received) = std::sync::mpsc::channel();
        engine.flush(move |_| {
            let _ = sent.send(());
        });
        received
            .recv_timeout(Duration::from_secs(5))
            .expect("the blur flushed");
        app.catch_up();
    }

    fn key_down(app: &mut Headless, key: &str, modifiers: Modifiers) {
        dispatch_key(app, key, true, modifiers, false);
    }

    fn key_up(app: &mut Headless, key: &str, modifiers: Modifiers) {
        dispatch_key(app, key, false, modifiers, false);
    }

    #[test]
    fn the_card_holds_focus_when_it_opens() {
        let mut app = start_keyboard_harness();
        app.wait_until("the card's focus", Duration::from_secs(5), |screen| {
            screen.matches("text:focus order: [Card]") == Ok(true)
        })
        .expect("the card takes the keyboard focus as it opens");
    }

    #[test]
    fn a_short_keyboard_press_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(100)).expect("held");
        key_up(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000))
            .expect("past the charge");
        assert_eq!(send_count(&app), 0, "a tap authorizes nothing");
        assert!(app.shows("Send for review?"), "the card stayed up");
    }

    #[test]
    fn a_continuous_keyboard_hold_sends_exactly_once() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000))
            .expect("the full charge");
        assert_eq!(send_count(&app), 1, "the full hold sent once");
        key_up(&mut app, "Enter", ctrl_enter());
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000)).expect("again");
        assert_eq!(send_count(&app), 1, "the spent hold never sends again");
    }

    #[test]
    fn a_keyup_at_550_ms_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(550)).expect("held");
        key_up(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(500))
            .expect("past 900 ms");
        assert_eq!(send_count(&app), 0, "the release cancelled the hold");
    }

    #[test]
    fn a_keyup_before_the_deadline_resets_the_charge_at_once() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(548)).expect("held");
        key_up(&mut app, "Enter", ctrl_enter());
        assert!(
            app.shows("Nothing was sent."),
            "the release reset the charge before the deadline: {:?}",
            app.screen().texts()
        );
        app.run_for(Duration::from_millis(400))
            .expect("past the first charge's 900 ms");
        assert_eq!(send_count(&app), 0, "nothing was sent");
    }

    #[test]
    fn ctrl_released_and_pressed_again_mid_hold_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(548)).expect("to 548 ms");
        dispatch_key(&mut app, "Control", false, Modifiers::default(), false);
        app.run_for(Duration::from_millis(8)).expect("to 556 ms");
        dispatch_key(&mut app, "Control", true, Modifiers::default(), false);
        app.run_for(Duration::from_millis(400))
            .expect("past the first hold's 900 ms");
        assert_eq!(
            send_count(&app),
            0,
            "a broken hold cannot authorize at its old deadline"
        );
    }

    #[test]
    fn a_fresh_enter_repress_restarts_the_charge_from_zero() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(600)).expect("to 600 ms");
        key_up(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(50)).expect("a beat");
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(400))
            .expect("past the first hold's 900 ms");
        assert_eq!(send_count(&app), 0, "the old hold never authorized");
        app.run_for(Duration::from_millis(600))
            .expect("the second hold's full charge");
        assert_eq!(send_count(&app), 1, "the new hold starts from zero");
    }

    #[test]
    fn an_opening_press_repeat_never_arms_the_card() {
        let mut app = start_keyboard_harness();
        // The `Ctrl+Enter` that opened the card never reached its handlers;
        // the auto-repeats that follow carry `repeat` and must not charge.
        dispatch_key(&mut app, "Enter", true, ctrl_enter(), true);
        app.run_for(Duration::from_millis(1_000))
            .expect("a long opening hold");
        assert_eq!(send_count(&app), 0, "a repeat never arms the card");
        // The release, then a fresh press: the card arms and the hold sends.
        key_up(&mut app, "Enter", ctrl_enter());
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000))
            .expect("the fresh hold");
        assert_eq!(
            send_count(&app),
            1,
            "the fresh press after the release charges"
        );
    }

    #[test]
    fn blur_mid_hold_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(400)).expect("held");
        // The focus leaves the card (a platform's own move, a window event):
        // the hold it had is gone.
        dispatch_blur(&mut app);
        app.run_for(Duration::from_millis(600))
            .expect("past 900 ms");
        assert_eq!(send_count(&app), 0, "a blurred card sends nothing");
    }

    #[test]
    fn releasing_command_mid_hold_sends_nothing() {
        // macOS reports Command's press and release as `Meta`: the hold must
        // end with the modifier, as it does with Control.
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", cmd_enter());
        app.run_for(Duration::from_millis(400)).expect("held");
        key_up(&mut app, "MetaLeft", Modifiers::default());
        app.run_for(Duration::from_millis(600))
            .expect("past 900 ms");
        assert_eq!(
            send_count(&app),
            0,
            "releasing Command ended the hold; Enter alone sends nothing"
        );
        // A fresh unbroken hold still sends.
        key_up(&mut app, "Enter", cmd_enter());
        key_down(&mut app, "Enter", cmd_enter());
        app.run_for(Duration::from_millis(1_000)).expect("again");
        assert_eq!(send_count(&app), 1, "a fresh hold sends once");
    }

    #[test]
    fn window_key_focus_loss_mid_hold_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(300)).expect("held");
        app.set_environment("the window lost the keyboard focus", |env| {
            let mut state = env.window_state.take().unwrap_or_default();
            state.focused = false;
            env.window_state = Some(state);
        })
        .expect("the key focus loss");
        app.run_for(Duration::from_millis(700))
            .expect("past 900 ms");
        assert_eq!(
            send_count(&app),
            0,
            "a window without the key focus sends nothing"
        );
        // Back with the key focus, a fresh hold still works.
        app.set_environment("the window has the keyboard focus again", |env| {
            if let Some(state) = env.window_state.as_mut() {
                state.focused = true;
            }
        })
        .expect("the key focus back");
        key_up(&mut app, "Enter", ctrl_enter());
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000))
            .expect("the fresh hold");
        assert_eq!(send_count(&app), 1, "the card works again with the focus");
    }

    #[test]
    fn tab_cycles_the_focus_inside_the_card() {
        let mut app = start_keyboard_harness();
        // From the card, forward reaches the first built control; the
        // platform's window-wide traversal never runs.
        app.press_key("tab").expect("tab");
        app.wait_until("the scope switch", Duration::from_secs(5), |screen| {
            screen.matches("text:focus order: [Card, Scope]") == Ok(true)
        })
        .expect("Tab reached the scope switch, not the window");
        for _ in 0..2 {
            app.press_key("tab").expect("tab");
        }
        app.wait_until("Send", Duration::from_secs(5), |screen| {
            screen.matches("text:focus order: [Scope, Cancel, Hold]") == Ok(true)
        })
        .expect("Tab reached Send");
        // From Send it wraps to a control of the card, never out of it.
        app.press_key("tab").expect("tab from Send");
        app.wait_until("a wrapped card control", Duration::from_secs(5), |screen| {
            screen.matches("text:focus order: [Cancel, Hold, Scope]") == Ok(true)
        })
        .expect("Tab wrapped inside the card");
        // Ctrl+Enter still reaches the card from a focused control, and a
        // full hold sends.
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000)).expect("the hold");
        assert_eq!(send_count(&app), 1, "the card still owns Ctrl+Enter");
    }

    #[test]
    fn window_deactivation_mid_hold_sends_nothing() {
        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(300)).expect("held");
        app.set_environment("the window lost the foreground", |env| {
            env.lifecycle = nsh_core::platform::Lifecycle::Inactive;
        })
        .expect("deactivation");
        app.run_for(Duration::from_millis(700))
            .expect("past 900 ms");
        assert_eq!(send_count(&app), 0, "a deactivated window sends nothing");
        // Back in front, a fresh hold still works.
        app.set_environment("the window is foreground again", |env| {
            env.lifecycle = nsh_core::platform::Lifecycle::Active;
        })
        .expect("activation");
        key_up(&mut app, "Enter", ctrl_enter());
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(1_000))
            .expect("the fresh hold");
        assert_eq!(send_count(&app), 1, "the card works again in front");
    }

    #[test]
    fn a_pointer_press_during_a_keyboard_hold_sends_nothing() {
        use nsh_core::geometry::point;

        let mut app = start_keyboard_harness();
        key_down(&mut app, "Enter", ctrl_enter());
        app.run_for(Duration::from_millis(600)).expect("to 600 ms");
        // The pointer takes over: the keyboard charge ends with its timer,
        // and the pointer's own charge starts from zero at 600 ms.
        app.drag_and_hold("id:consent-send", point(px(0.0), px(0.0)))
            .expect("pointer press at 600 ms");
        app.run_for(Duration::from_millis(300))
            .expect("past the keyboard hold's 900 ms");
        assert_eq!(
            send_count(&app),
            0,
            "the old timer cannot authorize a new path"
        );
        // The pointer's own hold runs from its press: its send lands at its
        // own deadline, never at the keyboard hold's.
        app.run_for(Duration::from_millis(600))
            .expect("the pointer's full charge");
        assert_eq!(send_count(&app), 1, "the pointer hold completed once");
        app.drop_held().expect("release the pointer");
    }

    #[test]
    fn two_short_enter_presses_on_send_send_nothing() {
        let mut app = start_keyboard_harness();
        for _ in 0..3 {
            app.press_key("tab").expect("tab to Send");
        }
        key_down(&mut app, "Enter", Modifiers::default());
        app.run_for(Duration::from_millis(500)).expect("to 500 ms");
        key_up(&mut app, "Enter", Modifiers::default());
        key_down(&mut app, "Enter", Modifiers::default());
        app.run_for(Duration::from_millis(400))
            .expect("past the first press's 900 ms");
        assert_eq!(
            send_count(&app),
            0,
            "two short presses never add up to one hold"
        );
    }

    #[test]
    fn space_on_the_focused_send_charges_and_its_release_cancels() {
        let mut app = start_keyboard_harness();
        for _ in 0..3 {
            app.press_key("tab").expect("tab to Send");
        }
        key_down(&mut app, " ", Modifiers::default());
        app.run_for(Duration::from_millis(500)).expect("held");
        key_up(&mut app, " ", Modifiers::default());
        app.run_for(Duration::from_millis(500))
            .expect("past 900 ms");
        assert_eq!(send_count(&app), 0, "a released Space sends nothing");
        // A full hold of the same key sends once.
        key_down(&mut app, " ", Modifiers::default());
        app.run_for(Duration::from_millis(1_000))
            .expect("the full charge");
        assert_eq!(send_count(&app), 1, "a full Space hold sends once");
    }

    #[test]
    fn pointer_split_presses_send_nothing() {
        use nsh_core::geometry::point;

        let mut app = start_harness();
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduced motion");
        app.drag_and_hold("id:consent-send", point(px(0.0), px(0.0)))
            .expect("down for 500 ms");
        app.run_for(Duration::from_millis(500)).expect("500 ms");
        app.drop_held().expect("up");
        app.run_for(Duration::from_millis(20)).expect("a beat");
        app.drag_and_hold("id:consent-send", point(px(0.0), px(0.0)))
            .expect("down again for 500 ms");
        app.run_for(Duration::from_millis(500)).expect("500 ms");
        app.drop_held().expect("up");
        app.run_for(Duration::from_millis(800))
            .expect("past both charges");
        assert!(!app.shows("Sent"), "split presses never add up");
    }
}
