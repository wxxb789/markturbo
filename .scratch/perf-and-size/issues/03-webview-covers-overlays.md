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
- Windows offers `SplitWeb` and keeps both workspace side panels in Web mode.
  Their resizable panes assign disjoint bounds, and GPUI prepaint sends only the
  preview pane's rectangle to the child HWND.
- Side-panel actions remain live in Web mode. Opening, closing, or focusing one
  schedules the same deferred WebView bounds update as any other layout change.
- GPUI still cannot interleave a popup with the browser inside the WebView
  rectangle. Web-active toolbar and tab chrome therefore remain fixed and
  overlay-free. The current editor search is pane-local, its context menu is a
  native Windows menu, and MarkTurbo installs no completion, hover, or code
  action provider. Adding one of those providers must reopen this boundary.
- Leaving Web mode restores GPUI focus on the main thread.

The earlier hide-on-overlay counter and companion-window experiments were
removed. Both solved one symptom by blanking or separating the preview and did
not satisfy the single-window product contract.

## Runtime proof

Command:

```text
printf '<h1>WebView layout probe</h1>\n' > probe-web.html
uv run scripts/probe.py windows \
  --exe target/release/markturbo.exe \
  --open probe-web.html \
  --settle 8 \
  --expect-top-level 1 \
  --expect-child-class WRY_WEBVIEW \
  --expect-native-chrome-insets \
  --resize-cycles 3 \
  --lifecycle-timeout 10
rm probe-web.html
```

Observed on August 26, 2026 after opening a temporary repository-root HTML file:

```text
application top-level windows  1
main client                    324,123  1400x900
MarkTurboWebHost               537,212   851x786  visible
WRY_WEBVIEW                    537,212   851x786  visible
native horizontal insets       213px left, 336px right
resize alternate/restore       6/6 PASS
WM_CLOSE                       exit 0 PASS
RefCell already borrowed       absent PASS
```

Windows also created four hidden, zero-sized `IME` / `MSCTFIME UI` helpers.
The probe reports them but excludes only those exact system input helpers from
the application-window count; any hidden product or non-zero top-level window
still fails.

The runtime probe covers the side-panel geometry and child-window lifecycle.
`SplitWeb` is guarded separately by the layout tests and by
`windows_split_web_has_no_floating_editor_provider`, which locks the current
no-floating-provider assumption behind the Windows compatibility decision.

## Guarded by

- `direct_composition_is_disabled_before_the_window_exists`
- `windows_uses_one_worker_owned_child_webview`
- `windows_publishes_only_a_ready_worker_and_never_joins_on_drop`
- `windows_prepaint_only_queues_bounds`
- `tab_affordances_cover_the_tab`
- `web_mode_keeps_fixed_chrome_and_side_panels`
- `web_mode_keeps_side_panel_actions_available`
- `windows_keeps_split_web_available`
- `windows_split_web_has_no_floating_editor_provider`
- `scripts/probe.py windows` runtime acceptance
