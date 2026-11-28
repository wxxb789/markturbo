//! Owner lane: 22-B (files and events).
//!
//! The drop screen: a toned paper panel that slides in from the right edge
//! while a file drag hovers over the window (Goal 22, decision 6).
//!
//! It takes no pointer — the drag belongs to the window — and it says only
//! what the drag's names and the event's types confirm
//! ([`crate::state::drop::DropOffer`]): the
//! prototype's "side being dragged over" is not required, and skipped items
//! are reported after the drop, by the drop's own notice.
//!
//! Motion: spatial, ease-out, from the motion tokens; interrupted freely, it
//! retargets from where it is. Under reduced motion the panel does not
//! translate at all — it fades.

use std::cell::Cell;

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    state::AppState,
    ui::{motion, prims, tokens},
};

/// The panel's width: it slides in from the right edge by its own width, as
/// the prototype's screen does.
const SCREEN_W: f32 = 320.0;

/// Whether the drop screen is up.
pub fn is_open(state: &AppState) -> bool {
    state.drop_offer.get_untracked().is_some()
}

/// The screen, mounted in the overlay layer so it slides over the whole
/// window. It stays out of the way (and off the screen) when no drag hovers.
pub fn screen(state: AppState, language: Language) -> impl IntoElement {
    let open_state = state.clone();
    let slide = Slide::new(move || open_state.drop_offer.get().is_some());
    when(
        move || slide.visible.get(),
        move || {
            let title_state = state.clone();
            view()
                .test_id("drop-screen")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .right(px(0.0))
                .w(px(SCREEN_W))
                .pointer_events(PointerEvents::None)
                .bg(move || tokens::palette().shoji_paper())
                .border(px(tokens::HAIRLINE))
                .border_color(move || tokens::palette().ink_caption)
                .px(px(tokens::SPACE_5))
                .py(px(tokens::SPACE_4))
                .flex_col()
                .justify_center()
                .gap(px(tokens::SPACE_2))
                .bind_translate_x(&slide.offset)
                .bind_opacity(&slide.opacity)
                .child(
                    text(move || {
                        title_state
                            .drop_offer
                            .get()
                            .map(|offer| offer.title(language))
                            .unwrap_or_default()
                    })
                    .test_id("drop-screen-title")
                    .font_family(tokens::FONT_MEDIUM)
                    .text_size(px(tokens::TYPE_SOURCE))
                    .line_height(px(tokens::TYPE_SOURCE_LINE))
                    .text_color(move || tokens::palette().ink),
                )
                .child(
                    view()
                        .test_id("drop-screen-hint")
                        .child(prims::rest(crate::state::drop::DropOffer::hint(language))),
                )
        },
    )
}

/// The panel's presence: a translation from behind the right edge plus an
/// opacity fade, or an opacity fade alone under reduced motion.
struct Slide {
    visible: Signal<bool>,
    offset: AnimatedValue,
    opacity: AnimatedValue,
}

impl Slide {
    fn new(open: impl Fn() -> bool + 'static) -> Self {
        let open = computed(open);
        let visible = signal(false);
        let offset = animated_value(0.0);
        let opacity = animated_value(0.0);
        let previous = Cell::new(None);
        let shown = Cell::new(false);
        let hide = Cell::new(None::<Spawned<()>>);
        effect(move || {
            let open = open.get();
            let reduced = motion::reduced_motion();
            let before = previous.replace(Some((open, reduced)));
            if before == Some((open, reduced)) {
                return;
            }
            hide.set(None);
            if open {
                visible.set(true);
                if reduced {
                    opacity.timing(1.0, motion::REDUCED_CROSSFADE_MS, motion::EASE_OUT);
                } else {
                    if !shown.replace(true) {
                        // The panel's first appearance: start fully behind
                        // the right edge. Later entrances continue from
                        // wherever a close left the panel.
                        offset.set(SCREEN_W);
                    }
                    opacity.timing(1.0, motion::ENTER_MS, motion::EASE_OUT);
                    offset.timing(0.0, motion::ENTER_MS, motion::EASE_OUT);
                }
            } else if reduced {
                opacity.set(0.0);
                visible.set(false);
            } else if visible.get_untracked() {
                let end = opacity.timing(0.0, motion::EXIT_MS, motion::EASE_OUT);
                offset.timing(SCREEN_W, motion::EXIT_MS, motion::EASE_OUT);
                hide.set(Some(spawn(async move {
                    if end.await {
                        visible.set(false);
                    }
                })));
            }
        });
        Self {
            visible,
            offset,
            opacity,
        }
    }
}
