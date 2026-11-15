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
//!   request — the endpoint, the items that leave with their own bytes and the
//!   framed remainder — and when no manifest exists it shows the state's own
//!   disclosure text verbatim, never summarized;
//! * the rows and the Total add up: the Total line prints exactly the sum of
//!   the rows above it (the framed bytes mt-core produced, when it produced
//!   them);
//! * Cancel has the default focus and equals the send's weight; Esc always
//!   cancels; a held Space or Enter (or two presses in quick succession)
//!   charges the hold, and a tap alone authorizes nothing;
//! * a send needs a deliberate hold: a press-and-release, a keyboard tap, an
//!   auto-repeat or a handler's second hand never authorize anything, and each
//!   consent is single-use;
//! * a loopback HTTP endpoint keeps its filament and row underlines plain grey
//!   and reads `HTTP (not encrypted)`.
//!
//! nsh 0.5.1 reports no key-up, so a held `Space` or `Enter` is read through
//! its auto-repeats: the first press starts the charge and every repeat (or
//! [`KEY_ALIVE_MS`] of silence) says whether the key is still down. A tap
//! therefore charges for one watchdog and cools back, while two presses in
//! quick succession carry the charge on.
//!
//! On Windows nsh does not move the keyboard focus to a view that is not a
//! text input, so the keyboard route cannot be conditioned on the hold
//! control's focus: `Space` and `Enter` start the hold while the card is up
//! and no text input has the focus (a keystroke that edits text never
//! charges). Cancel is still where the card asks for the focus, so an
//! assistive technology on a platform that focuses views activates the safe
//! choice.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use mt_core::{
    model::{EndpointIdentity, OutboundScopeKind, TransportEncryption},
    settings::Language,
};
use nsh_core::{Color, TransformKind, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, OutboundConsent, OutboundItem, OutboundOrigin},
    ui::{motion, prims, tokens},
};

/// The filament's height: a 2px bar along the send control's bottom edge.
const FILAMENT_H: f32 = 2.0;
/// The hold the control declares to the platform: the spec's ~1.2s, one third
/// longer than the charge so a held press always completes it.
const HOLD_MS: f32 = 1_200.0;
/// The card's rise into place, when the system allows motion.
const CARD_RISE_MS: f32 = 160.0;
/// How long a held key may stay silent before it counts as released: longer
/// than the platform's slowest first auto-repeat, shorter than the charge, so
/// a tap on its own can never complete a send.
const KEY_ALIVE_MS: f32 = 700.0;
/// How long 'Nothing was sent.' stays after an early release.
const NOTICE_MS: f32 = 1500.0;
/// A released filament is drawn cooling for three time constants.
const COOL_MS: f32 = 3.0 * motion::FILAMENT_RELEASE_TAU_MS;
/// The row height the height rule counts with (type plus its gap).
const ROW_H: f32 = 22.0;
/// What the card may take of the window, and the chrome below its rows.
const CARD_MAX_FRACTION: f32 = 0.80;
const CARD_CHROME_H: f32 = 320.0;
/// A tap must never complete a hold: the watchdog a key tap charges for stays
/// short of the charge a send needs.
const _: () = assert!(KEY_ALIVE_MS < motion::FILAMENT_CHARGE_MS);

/// Whether a consent is waiting: the shell dims the room and shows the card.
///
/// The manifests and the appearance are read here as well as the two consent
/// strings: the card is built from the structured manifest, so the shell must
/// re-key it when a scope switch replaces that manifest, and its few static
/// colours (the shadow, the filament's ramp) follow the theme.
pub fn is_open(state: &AppState) -> bool {
    let review = state.review_consent.get().is_some();
    let revision = state.revision_consent.get().is_some();
    let _ = state.review_manifest.get();
    let _ = state.revision_manifest.get();
    let _ = tokens::palette();
    review || revision
}

/// The consent the card shows.
struct Waiting {
    is_revision: bool,
    consent: Option<OutboundConsent>,
}

fn waiting(state: &AppState) -> Waiting {
    if state.review_consent.get().is_some() {
        return Waiting {
            is_revision: false,
            consent: state.review_manifest.get(),
        };
    }
    Waiting {
        is_revision: true,
        consent: state.revision_manifest.get(),
    }
}

/// The consent card.
pub fn card(state: AppState, language: Language) -> impl IntoElement {
    let waiting = waiting(&state);
    let is_revision = waiting.is_revision;
    // The app's only shadow: sending is the one thing that breaks the room.
    let shadow = shadow();
    // The 4px rise: a motion, so it is skipped under reduced motion.
    let rise = animated_value(tokens::CARD_RISE);
    effect(move || {
        if motion::reduced_motion() {
            rise.set(0.0);
        } else {
            rise.timing(0.0, CARD_RISE_MS, motion::rail_easing());
        }
    });
    let rows = waiting.consent.as_ref().map(|consent| {
        rows(
            consent,
            language,
            row_budget(window().height.0 * CARD_MAX_FRACTION),
        )
    });
    let total = rows.as_ref().map_or(0, |rows| rows.total);
    let hold = Hold::new(state.clone(), is_revision, language, total);
    // The state's own disclosure: shown verbatim when no manifest was kept.
    let disclosure = state
        .review_consent
        .get()
        .or_else(|| state.revision_consent.get())
        .unwrap_or_default();
    let mut card = view()
        .test_id("consent-card")
        .w(px(tokens::CONSENT_CARD_MAX_W))
        .flex_col()
        .gap(px(tokens::SPACE_4))
        .p(px(tokens::SPACE_5))
        .bg(move || tokens::palette().room)
        .shadow([shadow])
        .bind_translate_y(&rise)
        .child(
            text(if is_revision {
                S::HeadlineRevision.text(language)
            } else {
                S::HeadlineReview.text(language)
            })
            .font_family(tokens::FONT_UI)
            .font_weight(tokens::WEIGHT_MEDIUM)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink),
        );
    card = match waiting.consent.as_ref() {
        Some(consent) => card.child(disclosure_body(
            &state,
            language,
            &waiting,
            consent,
            rows.as_ref().expect("a manifest has its rows"),
        )),
        None => card.child(
            text(disclosure)
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        ),
    };
    // The keyboard: Esc always cancels; Enter and Space charge the hold. The
    // bindings live under the card's own owner (`run_owner`), so they go when
    // the card does.
    keyboard(state.clone(), is_revision, hold.clone());
    card.child(buttons(state, is_revision, language, hold, total))
}

/// The card's body: destination, scope, the rows that leave, the total and the
/// boundary statement.
fn disclosure_body(
    state: &AppState,
    language: Language,
    waiting: &Waiting,
    consent: &OutboundConsent,
    rows: &Rows,
) -> impl IntoElement {
    let loopback =
        consent.disclosure.endpoint().transport().encryption() == TransportEncryption::Unencrypted;
    let scope_state = state.clone();
    view()
        .test_id("consent-disclosure")
        .flex_col()
        .gap(px(tokens::SPACE_4))
        .child(prims::rest(SharedString::from(destination(
            language,
            consent.disclosure.endpoint(),
        ))))
        .child(scope_row(scope_state, language, waiting))
        .child(rows_section(language, rows, loopback))
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
                            text(SharedString::from(bytes_label(rows.total)))
                                .font_family(tokens::FONT_UI)
                                .text_size(px(tokens::TYPE_UI))
                                .line_height(px(tokens::TYPE_UI_LINE))
                                .text_color(move || tokens::palette().ink),
                        ),
                ),
        )
        .child(
            view()
                .flex_col()
                .child(prims::rest(S::BoundaryFraming.text(language)))
                .child(prims::rest(S::BoundaryContent.text(language))),
        )
}

/// The scope: the words the state can bind, the current one in full ink, the
/// other one a quiet switch (choosing a scope is your action, never warm).
fn scope_row(state: AppState, language: Language, waiting: &Waiting) -> impl IntoElement {
    let kind = waiting
        .consent
        .as_ref()
        .map(|consent| consent.disclosure.scope().kind());
    let with_context = kind == Some(OutboundScopeKind::DocumentWithEffectiveAgentContext);
    let selection = kind == Some(OutboundScopeKind::Selection);
    let offered = !waiting.is_revision && state.review_consent_offers_context();
    let to_context = state.clone();
    let to_document = state;
    let mut options: Vec<AnyElement> = Vec::new();
    if selection {
        options.push(scope_word(
            "consent-scope-selection",
            S::ScopeSelection.text(language),
        ));
    }
    options.push(scope_choice(
        "consent-scope-document",
        S::ScopeDocument.text(language),
        !with_context,
        (with_context).then(|| {
            Box::new(move || to_document.set_review_consent_scope(false)) as Box<dyn Fn()>
        }),
    ));
    if with_context || offered {
        options.push(scope_choice(
            "consent-scope-context",
            S::ScopeContext.text(language),
            with_context,
            (!with_context).then(|| {
                Box::new(move || to_context.set_review_consent_scope(true)) as Box<dyn Fn()>
            }),
        ));
    }
    view()
        .test_id("consent-scope")
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_3))
        .child(prims::rest(S::Scope.text(language)))
        .children(options)
}

/// The scope the disclosure carries, in full ink.
fn scope_word(test_id: &'static str, label: &'static str) -> AnyElement {
    view()
        .test_id(test_id)
        .child(prims::ink(label))
        .into_element()
}

/// One scope option: the current one is ink; an available other one is a quiet
/// local action; one that cannot be bound is not drawn.
fn scope_choice(
    test_id: &'static str,
    label: &'static str,
    selected: bool,
    on_press: Option<Box<dyn Fn()>>,
) -> AnyElement {
    if selected {
        return scope_word(test_id, label);
    }
    match on_press {
        Some(on_press) => view()
            .child(prims::quiet_action(test_id, label, on_press))
            .into_element(),
        None => view().test_id(test_id).into_element(),
    }
}

/// How a row is lit: outbound data is white-hot, a loopback endpoint is plain
/// grey, and what stays is unlit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Leaving,
    Loopback,
    Unlit,
}

/// The rows that leave, the framing row, then the items that stay.
fn rows_section(language: Language, rows: &Rows, loopback: bool) -> impl IntoElement {
    let tone = if loopback {
        Tone::Loopback
    } else {
        Tone::Leaving
    };
    let mut section = view()
        .test_id("consent-rows")
        .flex_col()
        .w_full()
        .gap(px(tokens::SPACE_1));
    if !rows.leaving.is_empty() || rows.framing.is_some() {
        section = section.child(prims::rest(S::LeavesThisDevice.text(language)));
        for row in rows.leaving.iter().chain(rows.framing.iter()) {
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

/// One manifest row: path, reason, bytes right-aligned, and the under-line —
/// white-hot as outbound data, plain grey for a loopback endpoint, ambient for
/// what stays.
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
    view()
        .test_id(test_id)
        .flex_col()
        .gap(px(tokens::SPACE_1))
        .child(prims::emitter_plate(
            view()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_2))
                .child(
                    text(label)
                        .font_family(tokens::FONT_UI)
                        .text_size(px(tokens::TYPE_UI))
                        .line_height(px(tokens::TYPE_UI_LINE))
                        .text_color(move || gloss().0),
                )
                .child(
                    text(reason)
                        .font_family(tokens::FONT_UI)
                        .text_size(px(tokens::TYPE_UI))
                        .line_height(px(tokens::TYPE_UI_LINE))
                        .text_color(move || gloss().1),
                )
                .child(view().flex_1())
                .child(
                    text(move || {
                        if has_bytes {
                            SharedString::from(bytes_label(bytes))
                        } else {
                            SharedString::from("")
                        }
                    })
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || gloss().0),
                ),
        ))
        .child(
            view()
                .h(px(tokens::HAIRLINE))
                .w_full()
                .bg(move || gloss().1),
        )
}

/// The two controls: Cancel (default focus, the send's weight) and the hold.
fn buttons(
    state: AppState,
    is_revision: bool,
    language: Language,
    hold: Hold,
    total: u64,
) -> impl IntoElement {
    view()
        .test_id("consent-buttons")
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_3))
        .child(cancel_button(state, is_revision, language))
        .child(send_button(hold, language, total))
}

/// Cancel: default focus, the send's weight, and it authorizes nothing.
fn cancel_button(state: AppState, is_revision: bool, language: Language) -> impl IntoElement {
    let reference = element_ref();
    // Cancel takes the keyboard focus as soon as the card is up, so Enter and
    // Escape both land somewhere that authorizes nothing.
    effect(move || reference.focus());
    view()
        .test_id("consent-cancel")
        .w(px(tokens::CONSENT_BUTTON_W))
        .h(px(tokens::CONSENT_BUTTON_H))
        .child(
            prims::primary_action(
                "consent-cancel-button",
                i18n::t(language, Key::Cancel),
                move || cancel(&state, is_revision),
            )
            .element_ref(&reference),
        )
}

/// Hold to send: a full [`motion::FILAMENT_CHARGE_MS`] of charge authorizes;
/// anything shorter authorizes nothing.
fn send_button(hold: Hold, language: Language, total: u64) -> impl IntoElement {
    let heat = hold.heat;
    let blur = hold.clone();
    let down = hold.clone();
    let up = hold.clone();
    let tap = hold.clone();
    let label_hold = hold.clone();
    view()
        .test_id("consent-send")
        .w(px(tokens::CONSENT_BUTTON_W))
        .h(px(tokens::CONSENT_BUTTON_H))
        .relative()
        .flex_col()
        .items_center()
        .justify_center()
        .border(px(tokens::HAIRLINE))
        .border_color(move || tokens::palette().ink_faint)
        .focus_visible(|s| s.border_color(tokens::palette().ink))
        .cursor_pointer()
        .focusable(true)
        .label(SharedString::from(S::HoldToSend.text(language)))
        // The window losing the keyboard ends any press in it.
        .on_blur(move || blur.release())
        // How long the platform calls a press on this control a hold: the
        // spec's ~1.2s, with the filament's [`motion::FILAMENT_CHARGE_MS`]
        // charge inside it.
        .long_press_delay(HOLD_MS)
        .on_press_down(move |_: &PressEvent| down.press())
        .on_press_up(move |_: &PressEvent| up.release())
        // An assistive technology's activation arrives as one whole press: the
        // press ended, so the charge cools and nothing is authorized.
        .on_press(move |_: &PressEvent| tap.release())
        .child(
            text(move || match label_hold.label.get() {
                HoldLabel::Idle => SharedString::from(hold_label(language, total)),
                HoldLabel::Holding => SharedString::from(S::KeepHolding.text(language)),
                HoldLabel::Released => SharedString::from(S::NothingWasSent.text(language)),
                HoldLabel::Sent => SharedString::from(S::Sent.text(language)),
            })
            .font_family(tokens::FONT_UI)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink),
        )
        .child(
            // The filament's track: the ramp's dull end, so the control shows
            // its filament before the hold starts.
            view()
                .absolute()
                .left(px(0.0))
                .bottom(px(0.0))
                .w_full()
                .h(px(FILAMENT_H))
                .bg(move || tokens::palette().filament[0]),
        )
        .child(
            // The charge itself: the ramp from dull to white-hot, as wide as
            // the hold.
            view()
                .absolute()
                .left(px(0.0))
                .bottom(px(0.0))
                .w_full()
                .h(px(FILAMENT_H))
                .origin_bottom_left()
                .bind_background(&heat.filament_glow())
                .bind_transform(TransformKind::ScaleX, &heat),
        )
}

/// The keyboard routes: Esc always cancels; a held `Enter` or `Space` charges
/// the hold (a keystroke that edits text never reaches these bindings).
fn keyboard(state: AppState, is_revision: bool, hold: Hold) {
    let escape_state = state.clone();
    let enter_state = state.clone();
    let space_state = state.clone();
    effect(move || {
        on_key_with("escape", KeyOptions::default(), {
            let state = escape_state.clone();
            move |_| cancel(&state, is_revision)
        });
        for key in ["enter", "space"] {
            let state = if key == "enter" {
                enter_state.clone()
            } else {
                space_state.clone()
            };
            let hold = hold.clone();
            on_key_with(
                key,
                KeyOptions {
                    repeat: true,
                    ..KeyOptions::default()
                },
                move |_| {
                    if !is_open(&state) {
                        return;
                    }
                    hold.key_press();
                },
            );
        }
    });
}

/// Cancel the waiting consent, whatever it is.
fn cancel(state: &AppState, is_revision: bool) {
    if is_revision {
        state.cancel_revision();
    } else {
        state.cancel_review();
    }
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
    /// The items whose content leaves, in display order, with any collapse row
    /// last.
    leaving: Vec<Row>,
    /// The framed bytes beyond the listed content, when mt-core framed the
    /// request.
    framing: Option<Row>,
    /// Package entries that leave as path, size and hash only: listed with
    /// their file size, but not part of the total — what they contribute at the
    /// boundary sits in the framing row.
    metadata: Vec<Row>,
    /// The items that stay.
    stays: Vec<Row>,
    /// The framed bytes mt-core produced, else the sum of the listed content.
    total: u64,
}

/// The rows of one consent, in the card's words. `budget` is how many content
/// rows the height rule leaves; the rest collapse into one summed row.
fn rows(consent: &OutboundConsent, language: Language, budget: usize) -> Rows {
    let mut leaving = Vec::new();
    let mut metadata = Vec::new();
    let mut content_bytes = 0_u64;
    let (mut shown, mut hidden, mut hidden_bytes) = (0_usize, 0_usize, 0_u64);
    // One more content row than the budget is cheaper than a collapse row for
    // a single file; beyond that the collapse row takes one of the budget's
    // rows.
    let content_rows = consent
        .items
        .iter()
        .filter(|item| item.content_leaves)
        .count();
    let budget = if content_rows > budget + 1 {
        budget.saturating_sub(1)
    } else {
        budget
    };
    for item in &consent.items {
        if item.content_leaves {
            content_bytes = content_bytes.saturating_add(item.bytes);
        }
        let row = Row {
            origin: item.origin,
            label: item
                .path
                .clone()
                .unwrap_or_else(|| name(language, item.origin).to_owned()),
            reason: reason_of(language, item),
            bytes: item.bytes,
            has_bytes: item.bytes > 0,
        };
        if item.content_leaves {
            if shown < budget {
                leaving.push(row);
                shown += 1;
            } else {
                hidden += 1;
                hidden_bytes = hidden_bytes.saturating_add(item.bytes);
            }
        } else {
            metadata.push(row);
        }
    }
    if hidden > 0 {
        leaving.push(Row {
            origin: OutboundOrigin::PackageFile,
            label: more_files(language, hidden),
            reason: String::new(),
            bytes: hidden_bytes,
            has_bytes: true,
        });
    }
    let framing = consent
        .framed_bytes
        .filter(|framed| *framed > content_bytes)
        .map(|framed| Row {
            origin: OutboundOrigin::Document,
            label: S::ProtocolFraming.text(language).to_owned(),
            reason: S::FramingReason.text(language).to_owned(),
            bytes: framed - content_bytes,
            has_bytes: true,
        });
    let total = consent.framed_bytes.unwrap_or(content_bytes);
    let stays = consent
        .stays
        .iter()
        .map(|item| Row {
            origin: item.origin,
            label: item
                .path
                .clone()
                .unwrap_or_else(|| name(language, item.origin).to_owned()),
            reason: reason_of(language, item),
            bytes: item.bytes,
            has_bytes: item.bytes > 0,
        })
        .collect();
    Rows {
        leaving,
        framing,
        metadata,
        stays,
        total,
    }
}

/// How many rows the height rule leaves in a card that may take `height`
/// points of the window.
fn row_budget(height: f32) -> usize {
    let rows = ((height - CARD_CHROME_H) / ROW_H).floor();
    if rows.is_finite() {
        (rows.clamp(2.0, 64.0)) as usize
    } else {
        2
    }
}

/// An item's own name when it has no path.
fn name(language: Language, origin: OutboundOrigin) -> &'static str {
    match origin {
        OutboundOrigin::Document => S::DocumentName.text(language),
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
        OutboundOrigin::NamedSource => S::NamedSourceReason.text(language).to_owned(),
        OutboundOrigin::PackageFile => S::PackageFileReason.text(language).to_owned(),
        OutboundOrigin::Omission => S::OmissionReason.text(language).to_owned(),
        OutboundOrigin::ExcludedSource => S::ExcludedSourceReason.text(language).to_owned(),
        OutboundOrigin::ReviewContext => S::ReviewContextReason.text(language).to_owned(),
        OutboundOrigin::Answers => S::AnswersReason.text(language).to_owned(),
    }
}

/// The destination line: where, how, and in what wire format.
fn destination(language: Language, endpoint: &EndpointIdentity) -> String {
    let path = endpoint.api_base_path().trim_end_matches('/');
    let address = if endpoint.transport().encryption() == TransportEncryption::Encrypted {
        format!(
            "{}:{} · HTTPS · {} {}",
            endpoint.host(),
            endpoint.effective_port(),
            endpoint.provider().label(),
            path
        )
    } else {
        format!(
            "{}:{} · {} · {} {}",
            endpoint.host(),
            endpoint.effective_port(),
            S::Unencrypted.text(language),
            endpoint.provider().label(),
            path
        )
    };
    format!("{} {}", S::To.text(language), address)
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

/// The hold control's label: how long to hold, and how much will leave.
fn hold_label(language: Language, bytes: u64) -> String {
    format!("{} {}", S::HoldToSend.text(language), bytes_label(bytes))
}

/// The collapse row: what did not fit, and its bytes.
fn more_files(language: Language, count: usize) -> String {
    match language {
        Language::English => format!("{count} more files"),
        Language::Chinese => format!("另外 {count} 个文件"),
    }
}

/// The card's shadow: the app's only one, 0 30px 80px at /.45 (dark) or /.14
/// (light).
fn shadow() -> BoxShadow {
    BoxShadow {
        offset_x: Points(0.0),
        offset_y: Points(30.0),
        blur: Points(80.0),
        spread: Points(0.0),
        color: if tokens::palette().is_dark {
            Color::rgba(0, 0, 0, 115)
        } else {
            Color::rgba(0, 0, 0, 36)
        },
        inset: false,
    }
}

// ─── the hold ─────────────────────────────────────────────────────────────

/// What the hold control's label says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldLabel {
    Idle,
    Holding,
    Released,
    Sent,
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

/// The hold time a charge still needs.
fn remaining_ms(charge: f32) -> f32 {
    (1.0 - charge).max(0.0) * motion::FILAMENT_CHARGE_MS
}

/// One hold: the filament's charge, the timers that decide when the charge is
/// full or a key was let go, and the single send it authorizes.
#[derive(Clone)]
struct Hold {
    state: AppState,
    is_revision: bool,
    language: Language,
    /// What leaves, as the `Sent · N bytes` line reports it.
    total: u64,
    /// What the control's label reads.
    label: Signal<HoldLabel>,
    /// The charge, 0..1, as the filament draws it.
    heat: AnimatedValue,
    /// The charge in progress: its value where the press began, and when.
    charging: Rc<Cell<Option<(f32, Instant)>>>,
    /// Where an early release left the charge, and when: the decay reads it.
    released: Rc<Cell<Option<(f32, Instant)>>>,
    /// The timer that completes the charge in progress.
    completion: Rc<RefCell<Option<Spawned<()>>>>,
    /// The watchdog that decides a held key was let go.
    key_alive: Rc<RefCell<Option<Spawned<()>>>>,
    /// The timer that ends the 'Nothing was sent.' notice.
    notice: Rc<RefCell<Option<Spawned<()>>>>,
    /// The send fired: this consent is spent.
    spent: Rc<Cell<bool>>,
}

impl Hold {
    fn new(state: AppState, is_revision: bool, language: Language, total: u64) -> Self {
        Self {
            state,
            is_revision,
            language,
            total,
            label: signal(HoldLabel::Idle),
            heat: animated_value(0.0),
            charging: Rc::new(Cell::new(None)),
            released: Rc::new(Cell::new(None)),
            completion: Rc::new(RefCell::new(None)),
            key_alive: Rc::new(RefCell::new(None)),
            notice: Rc::new(RefCell::new(None)),
            spent: Rc::new(Cell::new(false)),
        }
    }

    /// The charge now: while charging, what the press has added; while
    /// released, what was left, decayed.
    fn charge_now(&self) -> f32 {
        if let Some((value, at)) = self.charging.get() {
            return charged(value, at.elapsed().as_secs_f32() * 1000.0);
        }
        if let Some((value, at)) = self.released.get() {
            return cooled(value, at.elapsed().as_secs_f32() * 1000.0);
        }
        0.0
    }

    /// Begins or continues the charge, and completes it at once when it is
    /// already full.
    fn press(&self) {
        if self.spent.get() {
            return;
        }
        self.end_notice();
        self.cancel_completion();
        let value = self.charge_now();
        self.charging.set(Some((value, Instant::now())));
        self.released.set(None);
        if self.label.get() != HoldLabel::Holding {
            self.label.set(HoldLabel::Holding);
        }
        self.heat.stop();
        let remaining = remaining_ms(value);
        if remaining <= 0.0 {
            self.complete();
            return;
        }
        self.heat
            .timing_with(1.0, Timing::new(remaining, Easing::Linear));
        let hold = self.clone();
        let slot = self.completion.clone();
        *slot.borrow_mut() = Some(set_timeout(
            Duration::from_millis(remaining as u64),
            move || {
                hold.completion.borrow_mut().take();
                hold.complete();
            },
        ));
    }

    /// The press ended before the charge was full: cool back and say so.
    fn release(&self) {
        if self.spent.get() {
            return;
        }
        let Some((value, at)) = self.charging.take() else {
            return;
        };
        let value = charged(value, at.elapsed().as_secs_f32() * 1000.0);
        self.released.set(Some((value, Instant::now())));
        self.cancel_completion();
        self.cancel_key_alive();
        self.cool(value);
        self.label.set(HoldLabel::Released);
        let hold = self.clone();
        let slot = self.notice.clone();
        *slot.borrow_mut() = Some(set_timeout(
            Duration::from_millis(NOTICE_MS as u64),
            move || {
                hold.notice.borrow_mut().take();
                if hold.charging.get().is_none() && !hold.spent.get() {
                    hold.label.set(HoldLabel::Idle);
                }
            },
        ));
    }

    /// One keyboard press, or one of its auto-repeats: it charges, and the key
    /// counts as held until the repeats stop arriving.
    fn key_press(&self) {
        if self.spent.get() {
            return;
        }
        self.cancel_key_alive();
        self.press();
        let hold = self.clone();
        let slot = self.key_alive.clone();
        *slot.borrow_mut() = Some(set_timeout(
            Duration::from_millis(KEY_ALIVE_MS as u64),
            move || {
                hold.key_alive.borrow_mut().take();
                hold.release();
            },
        ));
    }

    /// The charge is full: authorize exactly once.
    fn complete(&self) {
        if self.spent.replace(true) {
            return;
        }
        self.charging.set(None);
        self.released.set(None);
        self.cancel_completion();
        self.cancel_key_alive();
        self.end_notice();
        self.heat.stop();
        self.heat.set(1.0);
        self.label.set(HoldLabel::Sent);
        self.state.status.set(SharedString::from(format!(
            "{} · {}",
            S::Sent.text(self.language),
            bytes_label(self.total)
        )));
        if self.is_revision {
            self.state.send_revision();
        } else {
            self.state.send_review();
        }
    }

    /// The drawn cooling of a released filament: the same exponential, sampled
    /// into one keyframe track from where the release left it.
    fn cool(&self, from: f32) {
        self.heat.stop();
        let frames: Vec<Keyframe> = (0..16)
            .map(|index| {
                let u = index as f32 / 15.0;
                Keyframe::at(u, cooled(from, u * COOL_MS)).ease(Easing::Linear)
            })
            .collect();
        let mut timing = KeyframeTiming::new(COOL_MS);
        timing.easing = Easing::Linear;
        self.heat.keyframes(frames, timing);
    }

    fn cancel_completion(&self) {
        if let Some(task) = self.completion.borrow_mut().take() {
            task.cancel();
        }
    }

    fn cancel_key_alive(&self) {
        if let Some(task) = self.key_alive.borrow_mut().take() {
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
        let ramp = tokens::palette().filament;
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
    To,
    Unencrypted,
    Scope,
    ScopeSelection,
    ScopeDocument,
    ScopeContext,
    LeavesThisDevice,
    StaysHere,
    MetadataOnly,
    ProtocolFraming,
    FramingReason,
    BoundaryFraming,
    BoundaryContent,
    Total,
    DocumentName,
    NamedSourceName,
    PackageFileName,
    OmissionName,
    ExcludedSourceName,
    ReviewContextName,
    AnswersName,
    DocumentReason,
    NamedSourceReason,
    PackageFileReason,
    OmissionReason,
    ExcludedSourceReason,
    ReviewContextReason,
    AnswersReason,
    NothingWasSent,
    KeepHolding,
    Sent,
    HoldToSend,
}

impl S {
    fn text(self, language: Language) -> &'static str {
        match language {
            Language::English => match self {
                S::HeadlineReview => "Send for review?",
                S::HeadlineRevision => "Send the revision?",
                S::To => "To",
                S::Unencrypted => "HTTP (not encrypted)",
                S::Scope => "Scope",
                S::ScopeSelection => "Selection",
                S::ScopeDocument => "Document",
                S::ScopeContext => "Document + context",
                S::LeavesThisDevice => "Leaves this device",
                S::StaysHere => "Stays here",
                S::MetadataOnly => "Path, size and hash only",
                S::ProtocolFraming => "Protocol framing",
                S::FramingReason => "the framed request",
                S::BoundaryFraming => "Protocol framing crosses this boundary.",
                S::BoundaryContent => "The disclosed source content also crosses it.",
                S::Total => "Total",
                S::DocumentName => "the document",
                S::NamedSourceName => "named source",
                S::PackageFileName => "package file",
                S::OmissionName => "not sent",
                S::ExcludedSourceName => "named source",
                S::ReviewContextName => "Review context",
                S::AnswersName => "Answers",
                S::DocumentReason => "the document",
                S::NamedSourceReason => "named source",
                S::PackageFileReason => "in the package",
                S::OmissionReason => "not sent with the package",
                S::ExcludedSourceReason => "not included",
                S::ReviewContextReason => "the review it builds on",
                S::AnswersReason => "your answers",
                S::NothingWasSent => "Nothing was sent.",
                S::KeepHolding => "Keep holding…",
                S::Sent => "Sent",
                S::HoldToSend => "Hold to send",
            },
            Language::Chinese => match self {
                S::HeadlineReview => "发送以进行审查？",
                S::HeadlineRevision => "发送修订？",
                S::To => "发送至",
                S::Unencrypted => "HTTP（未加密）",
                S::Scope => "范围",
                S::ScopeSelection => "选区",
                S::ScopeDocument => "文档",
                S::ScopeContext => "文档 + context",
                S::LeavesThisDevice => "离开此设备",
                S::StaysHere => "留在本机",
                S::MetadataOnly => "仅路径、大小与哈希",
                S::ProtocolFraming => "协议封装",
                S::FramingReason => "封装后的请求",
                S::BoundaryFraming => "协议封装会跨越此边界。",
                S::BoundaryContent => "上述源内容也会跨越此边界发送。",
                S::Total => "合计",
                S::DocumentName => "文档",
                S::NamedSourceName => "指定来源",
                S::PackageFileName => "包内文件",
                S::OmissionName => "未发送",
                S::ExcludedSourceName => "指定来源",
                S::ReviewContextName => "审查上下文",
                S::AnswersName => "回答",
                S::DocumentReason => "文档本身",
                S::NamedSourceReason => "指定来源",
                S::PackageFileReason => "包内文件",
                S::OmissionReason => "未随包发送",
                S::ExcludedSourceReason => "未包含",
                S::ReviewContextReason => "所依据的审查",
                S::AnswersReason => "你的回答",
                S::NothingWasSent => "未发送任何内容。",
                S::KeepHolding => "继续按住…",
                S::Sent => "已发送",
                S::HoldToSend => "按住发送",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_core::model::{ModelOperation, ModelRequestDisclosure, OutboundScope, Provider};
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

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
        // A document with one named source: the frame carries its tag, the
        // length prefixes and the context JSON beyond the content.
        let consent = OutboundConsent {
            disclosure: disclosure(OutboundScope::document(600)),
            framed_bytes: Some(1_000),
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 600, true),
                item(OutboundOrigin::NamedSource, "~/AGENTS.md", 200, true),
            ],
            stays: Vec::new(),
        };
        let rows = rows(&consent, Language::English, 20);
        let sum: u64 = rows.leaving.iter().map(|row| row.bytes).sum::<u64>()
            + rows.framing.as_ref().map_or(0, |row| row.bytes);
        assert_eq!(rows.total, 1_000);
        assert_eq!(sum, rows.total, "the printed total is the sum of the rows");
        assert_eq!(
            rows.framing.as_ref().map(|row| row.bytes),
            Some(200),
            "the framing row is the framed remainder"
        );
    }

    #[test]
    fn a_metadata_only_entry_is_listed_but_never_counted() {
        // A skill package with a binary asset: its raw bytes stay here; its
        // path, size and hash leave inside the frame.
        let consent = OutboundConsent {
            disclosure: disclosure(OutboundScope::document(2_140)),
            framed_bytes: Some(4_700),
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
        let rows = rows(&consent, Language::English, 20);
        let sum: u64 = rows.leaving.iter().map(|row| row.bytes).sum::<u64>()
            + rows.framing.as_ref().map_or(0, |row| row.bytes);
        assert_eq!(rows.total, 4_700);
        assert_eq!(sum, rows.total);
        assert_eq!(rows.metadata.len(), 1, "the binary is its own group");
        assert_eq!(rows.metadata[0].bytes, 12_004, "its disclosed size stays");
        assert!(
            !rows.leaving.iter().any(|row| row.label.contains("logo")),
            "its size is not part of the total"
        );
        assert_eq!(rows.stays.len(), 1);
    }

    #[test]
    fn a_revision_without_a_framed_total_lists_only_its_items() {
        let consent = OutboundConsent {
            disclosure: ModelRequestDisclosure::new(
                ModelOperation::Revision,
                EndpointIdentity::parse(Provider::OpenAiResponses, None).expect("endpoint"),
                OutboundScope::document(400),
            ),
            framed_bytes: None,
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 400, true),
                item(OutboundOrigin::ReviewContext, "Review context", 900, true),
                item(OutboundOrigin::Answers, "Answers", 120, true),
            ],
            stays: Vec::new(),
        };
        let rows = rows(&consent, Language::English, 20);
        assert_eq!(rows.total, 1_420);
        assert_eq!(rows.framing, None, "nothing framed is claimed");
    }

    #[test]
    fn overflow_collapses_into_a_row_that_keeps_the_total_whole() {
        let items: Vec<OutboundItem> = (0..8)
            .map(|index| {
                item(
                    OutboundOrigin::PackageFile,
                    &format!("file{index}.md"),
                    100,
                    true,
                )
            })
            .collect();
        let consent = OutboundConsent {
            disclosure: disclosure(OutboundScope::document(1)),
            framed_bytes: None,
            items,
            stays: Vec::new(),
        };
        let rows = rows(&consent, Language::English, 4);
        assert_eq!(rows.leaving.len(), 4, "three files and the collapse row");
        assert_eq!(rows.leaving[3].bytes, 500, "the hidden five, summed");
        let sum: u64 = rows.leaving.iter().map(|row| row.bytes).sum();
        assert_eq!(sum, rows.total);
    }

    #[test]
    fn a_tap_can_never_complete_a_hold() {
        // A keyboard tap charges for one watchdog at most; that must stay short
        // of the charge a send needs.
        let tapped = charged(0.0, KEY_ALIVE_MS);
        assert!(
            remaining_ms(tapped) > 0.0,
            "a single tap leaves charge to hold for"
        );
    }

    #[test]
    fn two_quick_presses_carry_the_charge_over_the_line() {
        let tapped = charged(0.0, KEY_ALIVE_MS);
        let between = cooled(tapped, 150.0);
        assert!(
            remaining_ms(between) < KEY_ALIVE_MS,
            "the second press reaches full charge before its watchdog"
        );
        assert!(
            remaining_ms(cooled(tapped, 2_000.0)) >= KEY_ALIVE_MS,
            "a slow second press is a fresh hold, not a carry-over"
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
        let line = destination(Language::English, &endpoint);
        assert!(
            line.starts_with(
                "To 127.0.0.1:8080 · HTTP (not encrypted) · OpenAI Responses /custom/v1"
            ),
            "{line}"
        );
    }

    /// A manifest of the shape the state builds: one document and one named
    /// source, framed at 1,000 bytes.
    fn sample_consent() -> OutboundConsent {
        OutboundConsent {
            disclosure: disclosure(
                OutboundScope::document_with_effective_agent_context(600, ["~/AGENTS.md"])
                    .expect("a named source"),
            ),
            framed_bytes: Some(1_000),
            items: vec![
                item(OutboundOrigin::Document, "plan.md", 600, true),
                item(OutboundOrigin::NamedSource, "~/AGENTS.md", 200, true),
            ],
            stays: Vec::new(),
        }
    }

    /// The card over a state that already has a consent waiting: the shell's
    /// `when(is_open)` around [`card`].
    #[component]
    fn Harness() -> impl IntoElement {
        let state = AppState::new();
        state
            .review_consent
            .set(Some(SharedString::from("disclosure text")));
        state.review_manifest.set(Some(sample_consent()));
        let card_state = state.clone();
        view().w(px(1_200.0)).h(px(800.0)).child(when(
            move || is_open(&state),
            move || card(card_state.clone(), Language::English),
        ))
    }

    pub(super) fn start_harness() -> (Headless, tempfile::TempDir) {
        let config = tempfile::tempdir().expect("config dir");
        // SAFETY: this test binary owns these variables; every test sets them.
        unsafe {
            std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
            std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        }
        // The system clock: the hold's charge, its watchdogs and the notice
        // are all real time, as they are in the app.
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<Harness>())).expect("start the harness");
        app.set_viewport(nsh_core::Size::new(Points(1_200.0), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:consent-card").expect("the card");
        (app, config)
    }

    #[test]
    fn the_card_shows_the_manifest_and_a_tap_authorizes_nothing() {
        let (mut app, _config) = start_harness();
        app.wait_for("id:consent-total").expect("the total");
        assert!(app.shows("Send for review?"), "the headline");
        assert!(app.shows("Leaves this device"), "the manifest caption");
        assert!(app.shows("plan.md"), "the document row");
        assert!(app.shows("~/AGENTS.md"), "the named source row");
        assert!(app.shows("1,000 B"), "the framed total");
        assert!(app.shows("Protocol framing"), "the framed remainder");
        assert!(app.shows("Hold to send 1,000 B"), "the hold's label");
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

    #[test]
    fn escape_cancels_and_closes_the_card() {
        let (mut app, _config) = start_harness();
        app.press_key("escape").expect("escape");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the card closed");
    }

    #[test]
    fn a_held_press_sends_once() {
        let (mut app, _config) = start_harness();
        app.long_press("id:consent-send").expect("hold to send");
        assert!(app.shows("Sent"), "a full hold authorized one send");
    }

    #[test]
    fn a_keyboard_tap_alone_sends_nothing() {
        let (mut app, _config) = start_harness();
        app.press_key("space").expect("space");
        // The watchdog ends the charge a tap started; no key-up is reported by
        // nsh, so this silence is the release.
        app.wait(Duration::from_millis(900))
            .expect("the watchdog ends the charge");
        assert!(!app.shows("Sent"), "a tap authorizes nothing");
    }
}
