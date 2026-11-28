//! The Afterglow UI.
//!
//! Owner lane: `foundation` owns this tree, [`shell`], [`tokens`], [`fonts`],
//! [`prims`] and [`motion`]. Every other file names its owner lane in its own
//! header and only that lane edits it.
//!
//! One window, one room. The shell composes the title row, the Shoji sidebar,
//! and the two-screen stage, plus the overlay layer for consent and notices.
//! The source editor stays mounted on one screen; the other holds one docked
//! surface at a time:
//!
//! ```text
//! revision  >  review  >  context  >  preview  >  nothing
//! ```
//!
//! A surface says whether it is on screen with its own `is_open(state)` and
//! draws itself with `field(state, language)`; [`settings`] replaces the whole
//! room with its page; [`notices`] contributes banners to the
//! column, the bottom status line and the overlay's guard.
//!
//! Only model output and outbound data emit light (see [`motion::Heat`] and
//! the emitters in [`prims`]); everything else is ambient ink. Local actions
//! never glow.

pub mod cjk_optics;
pub mod consent;
pub mod context;
pub mod document;
pub mod drop_screen;
pub mod fonts;
pub mod glow;
pub mod motion;
pub mod notices;
pub mod preview;
pub mod prims;
pub mod rail;
pub mod review;
pub mod revision;
pub mod settings;
pub mod shell;
pub mod titlebar;
pub mod tokens;
pub mod welcome;

pub use shell::shell;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nsh_core::{
        element::{effect, set_timeout},
        prelude::*,
    };
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;

    use super::{
        consent::{CardFocusStep, card_focus_step},
        glow::{TickerStep, ticker_step},
        review::{FocusStep, HintStep, focus_step, hint_step},
    };

    /// One step of each UI timer, armed together. While every timer shared one
    /// `global::<Option<Spawned<()>>>` slot, each `set` after the first
    /// cancelled the step before it.
    #[component]
    fn TimerSlots() -> impl IntoElement {
        let ticker = signal(0u32);
        let focus = signal(0u32);
        let hint = signal(0u32);
        let card = signal(0u32);
        effect(move || {
            // No owner: the handles alone keep the steps alive, as the global
            // slots did in production.
            Owner::detached().run(|| {
                ticker_step().set(TickerStep(Some(set_timeout(
                    Duration::from_millis(40),
                    move || ticker.update(|count| *count += 1),
                ))));
                focus_step().set(FocusStep(Some(set_timeout(
                    Duration::from_millis(50),
                    move || focus.update(|count| *count += 1),
                ))));
                hint_step().set(HintStep(Some(set_timeout(
                    Duration::from_millis(60),
                    move || hint.update(|count| *count += 1),
                ))));
                card_focus_step().set(CardFocusStep(Some(set_timeout(
                    Duration::from_millis(70),
                    move || card.update(|count| *count += 1),
                ))));
            });
        });
        view().test_id("timer-slots").child(text(move || {
            SharedString::from(format!(
                "{} {} {} {}",
                ticker.get(),
                focus.get(),
                hint.get(),
                card.get()
            ))
        }))
    }

    #[test]
    fn every_ui_timer_keeps_its_own_global_slot() {
        let mut slots = vec![
            std::any::TypeId::of::<TickerStep>(),
            std::any::TypeId::of::<CardFocusStep>(),
            std::any::TypeId::of::<FocusStep>(),
            std::any::TypeId::of::<HintStep>(),
        ];
        slots.sort();
        slots.dedup();
        assert_eq!(
            slots.len(),
            4,
            "one timer's write must not take another timer's slot"
        );
    }

    #[test]
    fn one_ui_timer_never_cancels_another_timers_step() {
        let mut app =
            Headless::start(EngineConfig::new(nsh_testing::windowed_root::<TimerSlots>()))
                .expect("start the timer slots");
        app.wait_for("text:1 1 1 1")
            .expect("every timer's step fired");
    }
}
