# 03 — The WebView covers every GPUI overlay in Web mode

Type: task
Status: resolved

## Question

Two problems reported against Web mode:

1. `ERROR gpui::window RefCell already borrowed` floods the log after switching
   to Web rendering.
2. After switching to Web mode, the WebView covers the layout dropdown, and the
   user cannot click it any more.

Are they still present, what causes each, and what is the fix?

## Answer

Both reproduce. They are unrelated to each other, and only the second is ours to
fix.

Reproduction for both is simply opening any `.html` file: `Layout::default_for`
returns `Layout::Web` for `DocType::Html`, so the WebView is created without a
click.

### Problem 1 — the log flood is upstream, and it is not only noise

The message is `BorrowMutError`'s `Display`, printed by `.log_err()`. The call
sites are in gpui's own frame loop — `window.rs:1604`, `:1627` and `:1639`, all
inside the `request_frame` callback, and all reaching the entity through
`handle.update(...)`, which bottoms out in the infallible `AppCell::borrow_mut`.

WebView2 pumps messages, so the window procedure re-enters while `App` is
already mutably borrowed for the draw. The borrow fails, and **that frame is
dropped**.

This is the same hazard this project already documented and worked around at
its own layer: `views/mod.rs` provides `try_update` / `try_update_in`, which
take the borrow through `with_window`'s `try_borrow_mut` precisely so a
re-entrant draw skips a frame rather than aborting. gpui's internal draw path
has no equivalent, and it is not reachable from here.

Not fixed. Fixing it means patching gpui.

Also seen and *not* a defect in this application:
`ERROR gpui_windows::window unable to get cursor position: Access is denied.
(0x80070005)` — that is this machine's session isolation, present with and
without a WebView.

### Problem 2 — the overlay is covered, and the cause is a deliberate trade

`main.rs` sets `GPUI_DISABLE_DIRECT_COMPOSITION=true` on Windows, with a comment
explaining why: GPUI composites its swap chain through DirectComposition, which
sits *on top of* the WebView2 child HWND, so the Web preview loads and paints
and is then covered by the window's own surface.

The cost of that trade is the exact reverse, and it was not written down: the
child window is now always above GPUI and does not participate in GPUI's
Z-order. Every overlay GPUI draws over the document — dropdown menu, context
menu, tooltip — is behind it, and the clicks land on the WebView.

Measured with an HTML document open in Web:

```
main window        317,123 to 1731,1030
WRY_WEBVIEW        552,202  912x796
Chrome_WidgetWin_0 552,202  912x796
Chrome_WidgetWin_1 552,202  912x796
Chrome_RenderWidgetHostHWND  552,202  912x796
Intermediate D3D Window      552,202  912x796
```

`WindowFromPoint` walking down the toolbar's column:

```
y=173  Zed::Window                    the toolbar
y=193  Zed::Window
y=198  Zed::Window                    last row GPUI owns
y=203  Chrome_RenderWidgetHostHWND    <- boundary
y=343  Chrome_RenderWidgetHostHWND
```

So the trigger button is clickable — it sits 19px above the child window — and
every row of the menu that opens beneath it is not.

**This is not specific to the layout dropdown.** Any GPUI overlay landing inside
the WebView's rectangle has the same problem.

### The fix

Hide the WebView while any overlay is open.

`WebSurface` gains an `overlays: usize` counter and `Workspace::overlay_changed`
increments and decrements it, marking the surface dirty only on the transitions
to and from zero. `webview_intent` returns `Hide` while the count is non-zero,
which routes through the same deferred, coalesced, fallibly-reached sync every
other intent change uses — so the re-entrancy rules on this module still hold.

A counter rather than a flag because overlays nest: a submenu opening over a
menu must not re-hide an already-hidden WebView, and the outer menu closing must
not reveal it while the inner one is still up. Both increments saturate, so a
close without a matching open cannot wrap the count to `usize::MAX` and hide the
preview for the rest of the session.

`gpui_wry::WebView::hide` also calls `focus_parent`, which is the other half of
the fix: without it the menu would be visible but the keyboard would still
belong to the browser process.

The layout dropdown had to change shape to report its state.
`Button::dropdown_menu` does not expose an open/close callback, so the same
control is now built from `Popover` — which does, via `on_open_change` — with
the identical `PopupMenu` content. The document emits
`DocumentEvent::OverlayOpen(bool)`, and the workspace acts on it, because the
WebView belongs to the window rather than to the tab.

### Measured

The same UIA-driven click test against both binaries. It finds the layout button
by accessibility name, clicks it, and asks `WindowFromPoint` who owns each menu
item's centre:

```
BEFORE the fix
  menu items visible after click: 2
    "Source" at 710,249 -> owner Chrome_RenderWidgetHostHWND
    "Web"    at 710,283 -> owner Chrome_RenderWidgetHostHWND

AFTER the fix
  menu items visible after click: 2
    "Source" at 710,249 -> owner Zed::Window
    "Web"    at 710,283 -> owner Zed::Window
```

Same coordinates, same two items, different owner. Before, the clicks go to the
browser; after, they go to GPUI.

The column scan agrees: before the click both binaries report
`Chrome_RenderWidgetHostHWND` from y+40 down; after the click the fixed binary
reports `Zed::Window` all the way down and the unfixed one does not change.

### The accepted cost

The preview blanks for as long as the menu is open.

The alternative considered was moving the child window off-screen instead of
hiding it, which would keep it rendering. Rejected: WebView2 treats an occluded
window as a reason to stop rendering anyway, so it trades a blank pane for a
stale one and adds coordinate arithmetic.

The real fix is to stop disabling DirectComposition and put both surfaces in one
Z-order — which is a change to the gpui/wry integration, not to this file. See
the map's Not yet specified.

### Guarded by

- `an_open_overlay_hides_the_webview` — the counter saturates, only transitions
  sync, and `webview_intent` checks the count.
- `the_layout_menu_reports_when_it_opens` — the toolbar reports its open state
  and does not use `dropdown_menu`, which would silently restore the bug.

Both source-level, because reproducing either failure needs a real WebView2
runtime and a real click.
