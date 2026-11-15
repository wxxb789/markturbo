//! Owner lane: consent.
//!
//! The hold-to-send card: the one place in the app that floats, and the one
//! thing that ever leaves the device.
//!
//! Contract: [`is_open`] says whether a consent is waiting (review, revision
//! or translation); [`card`] draws the card *inside* the shell's overlay —
//! the shell owns the overlay layer and the room's dimming, the card owns its
//! own size, its shadow (the app's only one) and the 4px rise.
//!
//! Trust rules this file must keep (the consent lane owns them): the
//! disclosure the state produced is shown verbatim and never summarized; the
//! destination line, the items that leave and the total all add up; Cancel has
//! the default focus and equals the send's weight; a send needs a deliberate
//! hold, so a press-release, a keyboard tap or a repeated key never authorizes
//! anything; Esc cancels; each consent is single-use.
//!
//! What is here now: the headline, the state's disclosure, Cancel, and a hold
//! that authorizes a send only after a full 900ms press. The consent lane adds
//! the filament's heat, the row manifest with its byte column, the scope
//! switch, the loopback label, keyboard holds (`Space`/`Enter` held), the
//! early-release message and the leaving light.

use std::{cell::RefCell, rc::Rc, time::Duration};

use mt_core::settings::Language;
use nsh_core::{
    Color, Points,
    element::{BoxShadow, PressEvent},
    prelude::*,
};

use crate::{
    state::AppState,
    ui::{
        motion::FILAMENT_CHARGE_MS,
        prims,
        tokens::{
            self, CONSENT_BUTTON_H, CONSENT_BUTTON_W, CONSENT_CARD_MAX_W, HAIRLINE, SPACE_3,
            SPACE_4, SPACE_5,
        },
    },
};

/// Whether a consent is waiting: the shell dims the room and shows the card.
pub fn is_open(state: &AppState) -> bool {
    state.review_consent.get().is_some() || state.revision_consent.get().is_some()
}

/// The consent card.
pub fn card(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    let disclosure_state = state.clone();
    let cancel_state = state.clone();
    let send_state = state.clone();
    let is_revision = state.revision_consent.get().is_some();
    // The app's only shadow: sending is the one thing that breaks the room.
    // `BoxShadow` takes plain colors, so it is built for the appearance the
    // card is built in.
    let shadow = BoxShadow {
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
    };
    view()
        .test_id("consent-card")
        .w(px(CONSENT_CARD_MAX_W))
        .flex_col()
        .gap(px(SPACE_4))
        .p(px(SPACE_5))
        .bg(move || tokens::palette().room)
        .shadow([shadow])
        .child(
            text(if is_revision {
                "Send the revision for consent?"
            } else {
                "Send for review?"
            })
            .font_family(tokens::FONT_UI)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink),
        )
        .child(
            text(move || {
                disclosure_state
                    .review_consent
                    .get()
                    .or_else(|| disclosure_state.revision_consent.get())
                    .unwrap_or_default()
            })
            .font_family(tokens::FONT_UI)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink_rest),
        )
        .child(
            view()
                .flex_row()
                .gap(px(SPACE_3))
                .child(cancel_button(cancel_state, is_revision))
                .child(send_button(send_state, is_revision)),
        )
}

/// Cancel: default focus, the send's weight, and it authorizes nothing.
fn cancel_button(state: AppState, is_revision: bool) -> impl IntoElement {
    let reference = element_ref();
    // Cancel takes the keyboard focus as soon as the card is up, so Enter and
    // Escape both land somewhere that authorizes nothing.
    effect(move || reference.focus());
    view()
        .w(px(CONSENT_BUTTON_W))
        .h(px(CONSENT_BUTTON_H))
        .child(
            prims::primary_action("consent-cancel", "Cancel", move || {
                if is_revision {
                    state.cancel_revision();
                } else {
                    state.cancel_review();
                }
            })
            .element_ref(&reference),
        )
}

/// Hold to send: a full press of `FILAMENT_CHARGE_MS` authorizes, anything
/// shorter authorizes nothing.
///
/// The control is a plain view so the press-down/up pair reaches it directly
/// (a `button()` speaks the component's own press events). The consent lane
/// adds the keyboard route — `Space` or `Enter` held — and the filament.
fn send_button(state: AppState, is_revision: bool) -> impl IntoElement {
    let pending: Rc<RefCell<Option<Spawned<()>>>> = Rc::new(RefCell::new(None));
    view()
        .test_id("consent-send")
        .w(px(CONSENT_BUTTON_W))
        .h(px(CONSENT_BUTTON_H))
        .flex_row()
        .items_center()
        .justify_center()
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().ink)
        .cursor_pointer()
        .focusable(true)
        .label(SharedString::from("Hold to send"))
        .on_press_down({
            let state = state.clone();
            let slot = pending.clone();
            move |_: &PressEvent| {
                let state = state.clone();
                let slot_for_task = slot.clone();
                let task = set_timeout(
                    Duration::from_millis(FILAMENT_CHARGE_MS as u64),
                    move || {
                        slot_for_task.borrow_mut().take();
                        if is_revision {
                            state.send_revision();
                        } else {
                            state.send_review();
                        }
                    },
                );
                *slot.borrow_mut() = Some(task);
            }
        })
        .on_press_up({
            let slot = pending.clone();
            move |_: &PressEvent| {
                if let Some(task) = slot.borrow_mut().take() {
                    task.cancel();
                }
            }
        })
        .child(prims::ink("Hold to send"))
}
