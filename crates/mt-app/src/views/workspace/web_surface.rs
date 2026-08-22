//! The window's single Web preview surface.
//!
//! One `WebView` per window, not per tab: it is an OS-level child window, so
//! the whole cluster exists to answer "which tab may hold it, showing what,
//! and when is it safe to touch". Split out of [`super::Workspace`] because
//! that answer was spread across three files, and the reasoning is the hardest
//! in the project.
//!
//! Three rules, each of which cost a real bug:
//!
//! 1. **Never touch it from `render`.** On Windows WebView2 pumps messages, so
//!    a mutation during a draw re-enters the window procedure while the `App`
//!    is already mutably borrowed and `AppCell::borrow_mut` panics. Everything
//!    here runs from [`Workspace::web_dirty`] through a `cx.defer`, which fires
//!    at the end of the effect cycle with no borrow held. The deferred callback
//!    still lands mid-draw sometimes, which is why it reaches the entity
//!    through the fallible windowed borrow described on
//!    [`crate::views::try_update_in`] rather than the infallible one.
//! 2. **Coalesce.** A theme change notifies every open document, so the naive
//!    version schedules one sync per tab. `sync_pending` is what turns a burst
//!    into one.
//! 3. **It must be *in* the active tab's element tree.** `WebViewElement::
//!    prepaint` is the only code that ever calls `set_bounds` on the child
//!    window, so a `WebView` that is alive but never rendered keeps the 0x0
//!    `Rect::default()` it was constructed with — loaded, visible, and
//!    invisible. [`Workspace::lend_webview`] hands it to exactly one tab and
//!    takes it back from every other; see `DocumentView::render_web_preview`
//!    for the other half.
//!
//! `gpui-wry` supports Windows and macOS only, so every field and every
//! function here is compiled away on Linux. What is *not* cfg'd is
//! [`Workspace::web_dirty`] and its dozen call sites — one `#[cfg]` inside
//! [`WebSurface::mark_dirty`] rather than twelve scattered across the caller.

use gpui::*;

use super::Workspace;

/// What the WebView should be showing.
///
/// `Unchanged` is not "do nothing because nothing happened" — it means the
/// answer is not known yet (a Web pane is wanted but its HTML has not been
/// built), and leaving the WebView alone beats flashing the previous document.
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WebIntent {
    Hide,
    Show(String),
    Unchanged,
}

/// The window's WebView and what it is currently showing.
///
/// Empty on Linux, where `gpui-wry` has no backend. It is still a field on the
/// workspace there, so the one `#[cfg]` needed lives in [`Self::mark_dirty`]
/// instead of at every caller.
#[derive(Debug, Default)]
pub(super) struct WebSurface {
    /// The single WebView for this window. It is an OS-level child window, so
    /// one is shared by every tab rather than created per document.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    webview: Option<Entity<gpui_wry::WebView>>,
    /// Set while a deferred sync is queued, so a burst of notifications
    /// coalesces into one.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    sync_pending: bool,
    /// What the WebView is currently showing, so we do not reload identical
    /// content on every frame.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    current: Option<String>,
}

impl WebSurface {
    /// Note that the WebView may need to change. Cheap and idempotent.
    ///
    /// The whole platform split is here rather than at the dozen call sites:
    /// on Linux this is the entire cluster, and it does nothing.
    fn mark_dirty(&mut self, cx: &mut Context<Workspace>) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            // Coalesced: a theme change notifies every open document, and one
            // sync per tab would apply the same intent N times.
            if self.sync_pending {
                return;
            }
            self.sync_pending = true;
            let this = cx.entity().downgrade();
            let entity_id = cx.entity_id();
            // **Never call the sync itself from `render`.** The WebView is an
            // OS child window driven by `wry`, and on Windows WebView2 pumps
            // messages: touching it re-enters the window procedure while the
            // `App` is already mutably borrowed for the draw, which panics in
            // `AppCell::borrow_mut`. `defer` runs the work at the end of the
            // effect cycle, with no borrow held.
            cx.defer(move |cx| {
                cx.with_window(entity_id, |window, cx| {
                    if this
                        .update(cx, |this, cx| {
                            this.web.sync_pending = false;
                            this.sync_webview(window, cx);
                        })
                        .is_err()
                    {
                        // Swallowing this left "the preview shows the previous
                        // document" with nothing in the log to attribute it to.
                        // The entity is gone, so the sync is moot — but say
                        // which one.
                        log::debug!("skipped a WebView sync: the workspace was released");
                    }
                });
            });
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = cx;
    }
}

impl Workspace {
    /// Note that the WebView may need to change. Cheap and idempotent.
    ///
    /// Every state change that can alter what the preview should show goes
    /// through here — a mode change, a reparse, a theme swap, a panel toggle,
    /// a tab focus. None of them carry a `#[cfg]`, because this does not
    /// either; [`WebSurface::mark_dirty`] is where the platform split lives.
    pub(super) fn web_dirty(&mut self, cx: &mut Context<Self>) {
        self.web.mark_dirty(cx);
    }

    /// What the WebView should be showing, as a pure read of current state.
    ///
    /// Separated from applying it because deciding is safe during a draw and
    /// applying is not — see [`Self::sync_webview`].
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_intent(&self, cx: &App) -> WebIntent {
        // The WebView is an OS child window drawn over GPUI's surface, not an
        // element in the tree — it would float on top of the settings page.
        if self.settings_open {
            return WebIntent::Hide;
        }
        let Some(doc) = self.active_document() else {
            return WebIntent::Hide;
        };
        let doc = doc.read(cx);
        if !doc.layout().uses_webview() {
            return WebIntent::Hide;
        }
        match doc.web_html() {
            Some(html) => WebIntent::Show(html.to_string()),
            // The mode wants the WebView but the HTML has not been built yet;
            // leaving it as-is avoids a flash of the previous document.
            None => WebIntent::Unchanged,
        }
    }

    /// Apply the current intent to the WebView. Must run outside a draw.
    ///
    /// Reached only from the `cx.defer` in [`WebSurface::mark_dirty`], which is
    /// what "outside a draw" means here.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn sync_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let intent = self.webview_intent(cx);

        let html = match intent {
            WebIntent::Unchanged => return,
            WebIntent::Hide => {
                if let Some(webview) = &self.web.webview {
                    webview.update(cx, |webview, _| webview.hide());
                }
                self.lend_webview(None, cx);
                self.web.current = None;
                return;
            }
            WebIntent::Show(html) => html,
        };

        let webview = match &self.web.webview {
            Some(webview) => webview.clone(),
            None => {
                let Some(webview) = create_webview(window, cx) else {
                    return;
                };
                self.web.webview = Some(webview.clone());
                webview
            }
        };

        // The WebView must be *in the active tab's element tree*, not merely
        // alive: its OS child window is positioned by `WebViewElement::prepaint`
        // and stays at the 0x0 `Rect::default()` it was built with otherwise.
        self.lend_webview(Some(webview.clone()), cx);

        webview.update(cx, |webview, _| webview.show());
        if self.web.current.as_deref() != Some(html.as_str()) {
            let url = crate::web::to_data_url(&html);
            webview.update(cx, |webview, _| webview.load_url(&url));
            self.web.current = Some(html);
        }
    }

    /// Give the WebView to the active tab and take it from every other one.
    ///
    /// Exactly one document may render it at a time — two would set conflicting
    /// bounds on the same child window every frame.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn lend_webview(&mut self, webview: Option<Entity<gpui_wry::WebView>>, cx: &mut Context<Self>) {
        let active = self.tabs.active_index();
        for (ix, doc) in self.document_views().into_iter().enumerate() {
            let lent = (ix == active).then(|| webview.clone()).flatten();
            doc.update(cx, |doc, cx| doc.set_webview(lent, cx));
        }
    }
}

/// Create the window's WebView.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn create_webview(window: &mut Window, cx: &mut App) -> Option<Entity<gpui_wry::WebView>> {
    use raw_window_handle::HasWindowHandle as _;

    let handle = window.window_handle().ok()?;
    let builder = wry::WebViewBuilder::new();
    #[cfg(debug_assertions)]
    let builder = builder.with_devtools(true);
    let webview = builder.build_as_child(&handle).ok()?;
    Some(cx.new(|cx| gpui_wry::WebView::new(webview, window, cx)))
}

#[cfg(test)]
mod tests {
    // Nothing is glob-imported: the `gpui::*` above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.

    /// `Workspace::render` must not mutate the WebView.
    ///
    /// This is a source-level check rather than a runtime one on purpose: the
    /// failure it guards is a `RefCell` panic that only reproduces when the
    /// platform re-enters the window procedure mid-draw (WebView2 pumping
    /// messages, a screen reader attaching). It is not reliably reachable from
    /// a test, but it is trivially reintroducible by someone "simplifying" the
    /// deferred sync back into `render` — which is exactly what this catches.
    ///
    /// It reads `workspace.rs` rather than this file: `render` lives there, and
    /// the point of the extraction is that nothing over there may reach in.
    #[test]
    fn render_does_not_touch_the_webview() {
        // `include_str!` resolves relative to this file at compile time, so it
        // works regardless of the test runner's working directory.
        let source = crate::views::production_source(include_str!("../workspace.rs"));
        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Render impl")
            .1;
        // Stop at the next top-level item so this only reads `render`'s body.
        let body = render.split("\n/// Keybindings").next().unwrap_or(render);

        for forbidden in ["sync_webview(", "webview.update(", "create_webview("] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` is called from render; the WebView is an OS child \
                 window and touching it during a draw re-enters the window \
                 procedure with the App already borrowed. Use `web_dirty` \
                 instead."
            );
        }
        // And the whole cluster stays here: `workspace.rs` may only ask for a
        // sync, never perform one.
        assert!(
            !source.contains("fn sync_webview"),
            "the sync belongs in views::workspace::web_surface, where the \
             re-entrancy rationale is"
        );
    }

    /// The sync must be deferred and coalesced, and reached fallibly.
    ///
    /// Three failures in one path, none of them reachable from a test. Calling
    /// `sync_webview` directly panics in `AppCell::borrow_mut` when the draw
    /// re-enters; dropping `sync_pending` runs one sync per notified document,
    /// and a theme change notifies all of them; and `Entity::update` on the
    /// deferred callback would abort the process instead of skipping a frame,
    /// because the callback can still land mid-draw.
    #[test]
    fn the_sync_is_deferred_coalesced_and_reached_fallibly() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source.find("fn mark_dirty").expect("mark_dirty must exist");
        let body = &source[start..];
        let end = body.find("\nimpl Workspace").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("cx.defer("),
            "the sync must run at the end of the effect cycle, with no borrow \
             held — calling it inline panics when the draw re-enters"
        );
        assert!(
            body.contains("if self.sync_pending") && body.contains("self.sync_pending = true"),
            "without the guard a theme change schedules one sync per open \
             document instead of one"
        );
        assert!(
            body.contains("cx.with_window(") && body.contains(".is_err()"),
            "the deferred callback can land mid-draw, so it must take the \
             window through `with_window`'s `try_borrow_mut` and report the \
             conflict rather than aborting"
        );
    }

    /// Only the active tab may hold the WebView.
    ///
    /// Two tabs childing one OS child window set conflicting bounds on it every
    /// frame; no tab childing it leaves it at the 0x0 `Rect::default()` it was
    /// constructed with, which is exactly "the Web view does not work".
    #[test]
    fn the_webview_is_lent_to_exactly_one_tab() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source
            .find("fn lend_webview")
            .expect("lend_webview must exist");
        let body = &source[start..];
        let end = body.find("\n}").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("(ix == active)"),
            "every non-active tab must be handed `None`, or two tabs fight over \
             the same child window's bounds"
        );
        assert!(
            body.contains("document_views()"),
            "the loop must cover every tab, not only the active one — a tab \
             that keeps a stale lease is the second half of that fight"
        );
        // Both intents go through it: hiding without taking the lease back
        // leaves the previous tab childing a hidden WebView.
        let sync = source.find("fn sync_webview").expect("sync_webview");
        let sync = &source[sync..start];
        assert_eq!(
            sync.matches("self.lend_webview(").count(),
            2,
            "Hide must take the lease back and Show must hand it over"
        );
    }
}
