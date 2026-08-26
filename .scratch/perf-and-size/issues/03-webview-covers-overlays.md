# 03 — Keep the WebView in one window without native-overlay conflicts

Type: task
Status: resolved

## Question

Web mode had two Windows failures:

1. tab tooltips, tab context menus, and the layout popup could be painted under
   the WebView child HWND and could not receive clicks;
2. WebView2 message pumping repeatedly logged `RefCell already borrowed` when
   Wry was mutated on GPUI's thread.

The preview must remain in the main application window. A companion top-level
window was prototyped and rejected because it split one document workflow into
two windows.

## Cause

GPUI's DirectComposition surface and an ordinary child HWND cannot interleave
per element. With DirectComposition enabled, GPUI covers the WebView. With the
compatibility compositor used by gpui-component's WebView example, the child
HWND is visible but remains above everything GPUI paints.

`ICoreWebView2CompositionController` could put the browser in the composition
tree, but a production integration also needs mouse, wheel, focus, IME, cursor,
touch, DPI, and lifecycle bridges. That is a new Windows WebView backend, not a
local Z-order fix.

## Resolution

The application keeps exactly one top-level window and makes the Windows
compatibility boundary explicit:

- `main.rs` selects `GPUI_DISABLE_DIRECT_COMPOSITION=true` before GPUI starts.
- A dedicated STA worker creates a private `WS_CHILD` `MarkTurboWebHost` inside
  the main HWND and builds Wry into that host.
- Every Wry operation stays on the worker. GPUI `prepaint` only sends resolved
  bounds through a channel.
- The worker publishes its handle only after WebView2 and its message queue are
  ready, blocks in `GetMessageW`, wakes through `PostThreadMessageW`, coalesces
  resize bursts, and never joins indefinitely from the GPUI thread.
- Web-active chrome creates no popup surface over the browser rectangle. Layout
  selection is fixed segmented chrome; tab tooltips and context menus exist
  only outside Web mode; the active path and copy commands are fixed in the
  title bar.
- Windows does not offer `SplitWeb`, because Editor-owned completion, search,
  hover, and context-menu overlays could cross into the child HWND.
- Side panels are not rendered in Web mode; their shortcuts report the required
  layout change in the fixed status bar instead of focusing hidden controls.
- Leaving Web mode restores GPUI focus on the main thread.

The earlier hide-on-overlay counter and companion-window experiments were
removed. Both solved one symptom by blanking or separating the preview and did
not satisfy the single-window product contract.

## Runtime proof

Command:

```text
uv run scripts/probe.py windows \
  --exe target/debug/markturbo.exe \
  --open target/probe-web.html \
  --settle 5 \
  --expect-top-level 1 \
  --expect-child-class WRY_WEBVIEW \
  --expect-native-chrome-insets \
  --resize-cycles 3 \
  --lifecycle-timeout 10
```

Observed on August 25, 2026:

```text
application top-level windows  1
MarkTurboWebHost               324,212  1400x786  visible
WRY_WEBVIEW                    324,212  1400x786  visible
resize alternate/restore       6/6 PASS
WM_CLOSE                       exit 0 PASS
RefCell already borrowed       absent PASS
```

Windows also created four hidden, zero-sized `IME` / `MSCTFIME UI` helpers.
The probe reports them but excludes only those exact system input helpers from
the application-window count; any hidden product or non-zero top-level window
still fails.

## Guarded by

- `direct_composition_is_disabled_before_the_window_exists`
- `windows_uses_one_worker_owned_child_webview`
- `windows_publishes_only_a_ready_worker_and_never_joins_on_drop`
- `windows_prepaint_only_queues_bounds`
- `tab_affordances_cover_the_tab`
- `web_mode_builds_only_fixed_overlay_free_chrome`
- `windows_never_places_an_editor_beside_the_child_webview`
- `scripts/probe.py windows` runtime acceptance
