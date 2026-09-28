//! GPUI navigation controls for the workspace's Back/Forward history.
//!
//! The history state and its invariants live in `mt_core::workspace::history`;
//! this module connects that state to document opening and renders its buttons.

use std::path::PathBuf;

use gpui_kit::component::{IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mt_core::workspace::history::Visit;

use super::{ChromeIconButton, NavigateBack, NavigateForward, Workspace};
use crate::i18n;

impl Workspace {
    /// Note that the user is now at `offset` in `path`.
    pub(super) fn record_visit(&mut self, path: PathBuf, offset: usize) {
        self.history.push(Visit { path, offset });
    }

    /// Go to `visit`, without recording the move as a new visit.
    fn go_to(&mut self, visit: Visit, window: &mut Window, cx: &mut Context<Self>) {
        // The flag is what keeps Back from truncating the forward half it is
        // walking through.
        self.history.set_navigating(true);
        self.open_file_as(visit.path.clone(), true, window, cx);
        if let Some(doc) = self.active_document().cloned()
            && doc.read(cx).source_path() == Some(visit.path.as_path())
        {
            doc.update(cx, |doc, cx| doc.reveal_offset(visit.offset, window, cx));
        }
        self.history.set_navigating(false);
        cx.notify();
    }

    pub(super) fn on_navigate_back(
        &mut self,
        _: &NavigateBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(visit) = self.history.back() {
            self.go_to(visit, window, cx);
        }
    }

    pub(super) fn on_navigate_forward(
        &mut self,
        _: &NavigateForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(visit) = self.history.forward() {
            self.go_to(visit, window, cx);
        }
    }

    /// Back and Forward, at the far left of the bar.
    ///
    /// A disabled button rather than a hidden one: the pair is a fixed landmark
    /// that the tabs start after, and one that appears and disappears would
    /// shift every tab sideways as the user navigates. Disabled says "there is
    /// nowhere to go" — which is the actual state — where absent says nothing.
    pub(super) fn render_navigator(&self, tooltips: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_0p5()
            // The bar is a `WindowControlArea::Drag` region, so a press here
            // becomes a window drag unless it is claimed back.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                ChromeIconButton::new(
                    "nav-back",
                    IconName::ArrowLeft,
                    i18n::t(i18n::Key::NavigateBack, cx),
                )
                .disabled(!self.history.can_go_back())
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::NavigateBack, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_navigate_back(&NavigateBack, window, cx)
                })),
            )
            .child(
                ChromeIconButton::new(
                    "nav-forward",
                    IconName::ArrowRight,
                    i18n::t(i18n::Key::NavigateForward, cx),
                )
                .disabled(!self.history.can_go_forward())
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::NavigateForward, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_navigate_forward(&NavigateForward, window, cx)
                })),
            )
    }
}
