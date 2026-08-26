//! The window's single Web preview surface.
//!
//! One WebView is shared by every tab. On macOS it stays in GPUI's existing
//! `gpui-wry` element path. On Windows it belongs to a dedicated STA worker:
//! WebView2 pumps messages while it is mutated, and doing that on GPUI's thread
//! can re-enter `AppCell` while a draw already holds the mutable borrow.

use gpui::*;

use super::Workspace;

#[cfg(target_os = "macos")]
use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::num::NonZeroIsize;
#[cfg(target_os = "macos")]
use std::rc::Rc;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex, mpsc};
#[cfg(target_os = "windows")]
use std::thread;

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WebPayloadKey {
    tab: usize,
    revision: u64,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingScroll {
    key: WebPayloadKey,
    fraction: f32,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Navigation {
    key: WebPayloadKey,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WebIntent {
    Hide,
    Show {
        key: WebPayloadKey,
        html: Option<String>,
    },
    Unchanged,
}

#[derive(Debug, Default)]
pub(super) struct WebSurface {
    #[cfg(target_os = "windows")]
    webview: Option<WindowsWebView>,
    #[cfg(target_os = "windows")]
    starting: bool,
    #[cfg(target_os = "macos")]
    webview: Option<Entity<gpui_wry::WebView>>,
    #[cfg(target_os = "macos")]
    navigation_in_flight: Rc<Cell<bool>>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    sync_pending: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    current: Option<WebPayloadKey>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pending_scroll: Option<PendingScroll>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    visible: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    lent_tab: Option<usize>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    loading: Option<Navigation>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    loaded: Option<Navigation>,
}

impl WebSurface {
    fn mark_dirty(&mut self, cx: &mut Context<Workspace>) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            if self.sync_pending {
                return;
            }
            self.sync_pending = true;
            let this = cx.entity().downgrade();
            let entity_id = cx.entity_id();
            cx.defer(move |cx| {
                cx.with_window(entity_id, |window, cx| {
                    if this
                        .update(cx, |this, cx| {
                            this.web.sync_pending = false;
                            this.sync_webview(window, cx);
                        })
                        .is_err()
                    {
                        log::debug!("skipped a WebView sync: the workspace was released");
                    }
                });
            });
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = cx;
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn payload_update(&self, key: WebPayloadKey, html: &str) -> Option<String> {
        (self.loading.is_none() && self.current != Some(key)).then(|| html.to_string())
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn begin_hide(&mut self) -> bool {
        let was_visible = self.visible;
        self.visible = false;
        was_visible
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn begin_navigation(&mut self, key: WebPayloadKey) -> Navigation {
        debug_assert!(self.loading.is_none(), "navigations are serialized");
        let navigation = Navigation { key };
        if let Some(pending) = &mut self.pending_scroll {
            if pending.key.tab == key.tab {
                pending.key = key;
            } else {
                self.pending_scroll = None;
            }
        }
        self.current = Some(key);
        self.loading = Some(navigation);
        navigation
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn finish_navigation(&mut self) -> Option<Navigation> {
        let navigation = self.loading.take()?;
        self.loaded = Some(navigation);
        Some(navigation)
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn ready_scroll(&self, key: WebPayloadKey) -> Option<f32> {
        if self.loading.is_some()
            || !self.loaded.is_some_and(|navigation| navigation.key == key)
            || !self
                .pending_scroll
                .is_some_and(|pending| pending.key == key)
        {
            return None;
        }
        self.pending_scroll.map(|pending| pending.fraction)
    }
}

impl Workspace {
    pub(super) fn web_dirty(&mut self, cx: &mut Context<Self>) {
        self.web.mark_dirty(cx);
    }

    pub(super) fn queue_web_scroll(&mut self, fraction: f32, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let tab = self.tabs.active_index();
            let Some(document) = self.active_document() else {
                return;
            };
            let document = document.read(cx);
            if !document.layout().uses_webview() {
                return;
            }
            let Some((_, revision)) = document.web_payload() else {
                return;
            };
            self.web.pending_scroll = Some(PendingScroll {
                key: WebPayloadKey { tab, revision },
                fraction,
            });
            self.web_dirty(cx);
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = (fraction, cx);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_intent(&self, cx: &App) -> WebIntent {
        if self.settings_open {
            return WebIntent::Hide;
        }
        let tab = self.tabs.active_index();
        let Some(doc) = self.active_document() else {
            return WebIntent::Hide;
        };
        let doc = doc.read(cx);
        if !doc.layout().uses_webview() {
            return WebIntent::Hide;
        }
        match doc.web_payload() {
            Some((html, revision)) => {
                let key = WebPayloadKey { tab, revision };
                WebIntent::Show {
                    key,
                    html: self.web.payload_update(key, html),
                }
            }
            None => WebIntent::Unchanged,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn sync_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (key, html) = match self.webview_intent(cx) {
            WebIntent::Unchanged => return,
            WebIntent::Hide => {
                let was_visible = self.web.begin_hide();
                #[cfg(target_os = "windows")]
                if was_visible && !focus_native_window(window) {
                    log::debug!("failed to restore native focus before hiding Web preview");
                }
                if was_visible
                    && let Some(webview) = &self.web.webview
                    && !hide_webview(webview, cx)
                {
                    self.webview_connection_lost("hiding", cx);
                }
                if self.web.lent_tab.take().is_some() {
                    self.lend_webview(None, None, cx);
                }
                self.web.pending_scroll = None;
                if was_visible {
                    // Only the visible -> hidden transition owns this focus
                    // handoff. Repeated Hide syncs must not steal focus back
                    // from an editor the user focused after the first one.
                    window.focus(&self.focus_handle, cx);
                }
                return;
            }
            WebIntent::Show { key, html } => (key, html),
        };

        let webview = match &self.web.webview {
            Some(webview) => webview.clone(),
            None => {
                #[cfg(target_os = "windows")]
                {
                    self.start_windows_webview(window, cx);
                    return;
                }
                #[cfg(target_os = "macos")]
                {
                    let navigation_in_flight = Rc::new(Cell::new(false));
                    let (page_loaded, page_events) = smol::channel::unbounded();
                    let Some(webview) =
                        create_webview(window, navigation_in_flight.clone(), page_loaded, cx)
                    else {
                        return;
                    };
                    self.web.navigation_in_flight = navigation_in_flight;
                    self.web.webview = Some(webview.clone());
                    let this = cx.entity().downgrade();
                    cx.spawn(async move |_, cx| {
                        while page_events.recv().await.is_ok() {
                            crate::views::try_update(&this, cx, |this, cx| {
                                this.webview_page_loaded(cx);
                            });
                        }
                    })
                    .detach();
                    webview
                }
            }
        };

        if self.web.lent_tab != Some(key.tab) {
            self.lend_webview(Some(key.tab), Some(webview.clone()), cx);
            self.web.lent_tab = Some(key.tab);
        }
        if !self.web.visible {
            if !show_webview(&webview, cx) {
                self.webview_connection_lost("showing", cx);
                return;
            }
            self.web.visible = true;
        }

        if let Some(html) = html {
            let navigation = self.web.begin_navigation(key);
            #[cfg(target_os = "windows")]
            let _ = navigation;
            #[cfg(target_os = "macos")]
            self.web.navigation_in_flight.set(true);
            if !load_webview(&webview, crate::web::to_data_url(&html), cx) {
                #[cfg(target_os = "macos")]
                self.web.navigation_in_flight.set(false);
                self.webview_connection_lost("navigating", cx);
                return;
            }
            #[cfg(target_os = "macos")]
            let _ = navigation;
        }

        if let Some(fraction) = self.web.ready_scroll(key) {
            if !evaluate_webview(&webview, scroll_script(fraction), cx) {
                self.webview_connection_lost("synchronizing scroll", cx);
                return;
            }
            self.web.pending_scroll = None;
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_connection_lost(&mut self, operation: &str, cx: &mut Context<Self>) {
        self.web.webview = None;
        self.web.current = None;
        self.web.visible = false;
        self.web.loading = None;
        self.web.loaded = None;
        #[cfg(target_os = "macos")]
        {
            self.web.navigation_in_flight = Rc::new(Cell::new(false));
        }
        #[cfg(target_os = "windows")]
        {
            self.web.starting = false;
        }
        if self.web.lent_tab.take().is_some() {
            self.lend_webview(None, None, cx);
        }
        self.set_status(
            format!("Web preview worker disconnected while {operation}; restarting"),
            cx,
        );
        self.web_dirty(cx);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_page_loaded(&mut self, cx: &mut Context<Self>) {
        if self.web.finish_navigation().is_some() {
            self.web_dirty(cx);
        }
    }

    #[cfg(target_os = "windows")]
    fn start_windows_webview(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.web.starting {
            return;
        }
        let Some(startup) = WindowsWebView::start(window) else {
            self.set_status("Cannot start the Web preview worker".into(), cx);
            return;
        };
        self.web.starting = true;
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx.background_spawn(async move { startup.wait() }).await;
            let ready = match result {
                Ok(ready) => ready,
                Err(error) => {
                    crate::views::try_update(&this, cx, |this, cx| {
                        this.web.starting = false;
                        this.web.current = None;
                        this.set_status(format!("Cannot start Web preview: {error}"), cx);
                    });
                    return;
                }
            };
            let worker_id = ready.webview.identity();
            let worker = ready.webview;
            crate::views::try_update(&this, cx, |this, cx| {
                this.web.starting = false;
                this.web.webview = Some(worker);
                this.web_dirty(cx);
            });

            while let Ok(event) = ready.events.recv().await {
                crate::views::try_update(&this, cx, |this, cx| {
                    this.handle_webview_event(worker_id, event, cx);
                });
            }
            crate::views::try_update(&this, cx, |this, cx| {
                if this
                    .web
                    .webview
                    .as_ref()
                    .is_some_and(|worker| worker.identity() == worker_id)
                {
                    this.webview_connection_lost("waiting for page events", cx);
                }
            });
        })
        .detach();
    }

    #[cfg(target_os = "windows")]
    fn handle_webview_event(
        &mut self,
        worker_id: usize,
        event: WorkerEvent,
        cx: &mut Context<Self>,
    ) {
        if !self
            .web
            .webview
            .as_ref()
            .is_some_and(|worker| worker.identity() == worker_id)
        {
            return;
        }
        match event {
            WorkerEvent::PageLoaded => self.webview_page_loaded(cx),
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn lend_webview(
        &mut self,
        tab: Option<usize>,
        webview: Option<PlatformWebView>,
        cx: &mut Context<Self>,
    ) {
        for (ix, doc) in self.document_views().into_iter().enumerate() {
            let lent = (Some(ix) == tab).then(|| webview.clone()).flatten();
            doc.update(cx, |doc, cx| doc.set_webview(lent, cx));
        }
    }
}

#[cfg(target_os = "windows")]
type PlatformWebView = WindowsWebView;
#[cfg(target_os = "macos")]
type PlatformWebView = Entity<gpui_wry::WebView>;

#[cfg(target_os = "macos")]
fn create_webview(
    window: &mut Window,
    navigation_in_flight: Rc<Cell<bool>>,
    page_loaded: smol::channel::Sender<()>,
    cx: &mut App,
) -> Option<Entity<gpui_wry::WebView>> {
    use raw_window_handle::HasWindowHandle as _;

    let handle = window.window_handle().ok()?;
    let builder = wry::WebViewBuilder::new().with_on_page_load_handler(move |event, _url| {
        if matches!(event, wry::PageLoadEvent::Finished) && navigation_in_flight.replace(false) {
            let _ = page_loaded.try_send(());
        }
    });
    #[cfg(debug_assertions)]
    let builder = builder.with_devtools(true);
    let webview = builder.build_as_child(&handle).ok()?;
    Some(cx.new(|cx| gpui_wry::WebView::new(webview, window, cx)))
}

#[cfg(target_os = "windows")]
fn hide_webview(webview: &WindowsWebView, _: &mut App) -> bool {
    webview.send(WorkerCommand::Hide).is_ok()
}

#[cfg(target_os = "windows")]
fn focus_native_window(window: &Window) -> bool {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);

    // Wry's parent is the worker-owned WebHost, so `focus_parent` would leave
    // native focus on a child that is about to be hidden. Run this on GPUI's
    // thread and hand focus directly to the application's main HWND instead.
    unsafe {
        let _ = SetFocus(Some(hwnd));
        GetFocus() == hwnd
    }
}

#[cfg(target_os = "macos")]
fn hide_webview(webview: &Entity<gpui_wry::WebView>, cx: &mut App) -> bool {
    webview.update(cx, |webview, _| webview.hide());
    true
}

#[cfg(target_os = "windows")]
fn show_webview(webview: &WindowsWebView, _: &mut App) -> bool {
    webview.send(WorkerCommand::Show).is_ok()
}

#[cfg(target_os = "macos")]
fn show_webview(webview: &Entity<gpui_wry::WebView>, cx: &mut App) -> bool {
    webview.update(cx, |webview, _| webview.show());
    true
}

#[cfg(target_os = "windows")]
fn load_webview(webview: &WindowsWebView, url: String, _: &mut App) -> bool {
    webview.send(WorkerCommand::LoadUrl(url)).is_ok()
}

#[cfg(target_os = "macos")]
fn load_webview(webview: &Entity<gpui_wry::WebView>, url: String, cx: &mut App) -> bool {
    webview.update(cx, |webview, _| webview.load_url(&url));
    true
}

#[cfg(target_os = "windows")]
fn evaluate_webview(webview: &WindowsWebView, script: String, _: &mut App) -> bool {
    webview.send(WorkerCommand::Evaluate(script)).is_ok()
}

#[cfg(target_os = "macos")]
fn evaluate_webview(webview: &Entity<gpui_wry::WebView>, script: String, cx: &mut App) -> bool {
    webview.update(cx, |webview, _| {
        let _ = webview.raw().evaluate_script(&script);
    });
    true
}

fn scroll_script(fraction: f32) -> String {
    format!(
        "(function(){{var e=document.scrollingElement||document.body;\
         if(!e)return;var h=e.scrollHeight-e.clientHeight;\
         if(h>0)e.scrollTop=h*{fraction};}})()"
    )
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PhysicalBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[cfg(target_os = "windows")]
enum WorkerCommand {
    Show,
    Hide,
    LoadUrl(String),
    Evaluate(String),
    Bounds(PhysicalBounds),
    Shutdown,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerEvent {
    PageLoaded,
}

#[cfg(target_os = "windows")]
const WORKER_WAKE_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x4d;

#[cfg(target_os = "windows")]
struct WorkerConnection {
    tx: mpsc::Sender<WorkerCommand>,
    last_bounds: Mutex<Option<PhysicalBounds>>,
    thread_id: u32,
}

#[cfg(target_os = "windows")]
impl Drop for WorkerConnection {
    fn drop(&mut self) {
        if self.tx.send(WorkerCommand::Shutdown).is_ok() {
            let _ = wake_worker(self.thread_id);
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsWebViewStartup {
    tx: mpsc::Sender<WorkerCommand>,
    ready: mpsc::Receiver<Result<u32, String>>,
    events: smol::channel::Receiver<WorkerEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
struct WindowsWebViewReady {
    webview: WindowsWebView,
    events: smol::channel::Receiver<WorkerEvent>,
}

#[cfg(target_os = "windows")]
impl WindowsWebViewStartup {
    fn wait(mut self) -> Result<WindowsWebViewReady, String> {
        match self.ready.recv() {
            Ok(Ok(thread_id)) => {
                // Dropping a successful JoinHandle detaches it. Shutdown is
                // driven by the connection's thread-message wake, never by an
                // unbounded join on GPUI's UI thread.
                self.thread.take();
                Ok(WindowsWebViewReady {
                    webview: WindowsWebView(Arc::new(WorkerConnection {
                        tx: self.tx,
                        last_bounds: Mutex::new(None),
                        thread_id,
                    })),
                    events: self.events,
                })
            }
            Ok(Err(error)) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Err(error)
            }
            Err(_) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Err("the Web preview worker exited before it became ready".into())
            }
        }
    }
}

/// A cloneable lease on the worker-owned Windows WebView.
#[cfg(target_os = "windows")]
#[derive(Clone)]
pub(crate) struct WindowsWebView(Arc<WorkerConnection>);

#[cfg(target_os = "windows")]
impl std::fmt::Debug for WindowsWebView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsWebView")
            .finish_non_exhaustive()
    }
}

#[cfg(target_os = "windows")]
impl PartialEq for WindowsWebView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(target_os = "windows")]
impl Eq for WindowsWebView {}

#[cfg(target_os = "windows")]
impl WindowsWebView {
    fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    fn start(window: &Window) -> Option<WindowsWebViewStartup> {
        use raw_window_handle::RawWindowHandle;

        let parent = match raw_window_handle::HasWindowHandle::window_handle(window)
            .ok()?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => handle.hwnd.get(),
            _ => return None,
        };
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (event_tx, events) = smol::channel::unbounded();
        let worker = thread::Builder::new()
            .name("markturbo-webview-sta".into())
            .spawn(move || run_windows_webview(parent, rx, ready_tx, event_tx))
            .ok()?;
        Some(WindowsWebViewStartup {
            tx,
            ready,
            events,
            thread: Some(worker),
        })
    }

    fn send(&self, command: WorkerCommand) -> Result<(), ()> {
        if self.0.tx.send(command).is_err() {
            log::debug!("skipped a WebView command: the worker was released");
            return Err(());
        }
        wake_worker(self.0.thread_id)
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>, scale_factor: f32) {
        let bounds = bounds.to_device_pixels(scale_factor);
        let bounds = PhysicalBounds {
            x: bounds.origin.x.0,
            y: bounds.origin.y.0,
            width: bounds.size.width.0.max(0),
            height: bounds.size.height.0.max(0),
        };
        let Ok(mut current) = self.0.last_bounds.lock() else {
            return;
        };
        if current.as_ref() == Some(&bounds) {
            return;
        }
        *current = Some(bounds);
        drop(current);
        let _ = self.send(WorkerCommand::Bounds(bounds));
    }
}

#[cfg(target_os = "windows")]
fn wake_worker(thread_id: u32) -> Result<(), ()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

    // SAFETY: the ready handshake is sent only after the worker has created its
    // message queue. A failed post means the worker has already exited.
    if let Err(error) =
        unsafe { PostThreadMessageW(thread_id, WORKER_WAKE_MESSAGE, WPARAM(0), LPARAM(0)) }
    {
        log::debug!("failed to wake Web preview worker: {error}");
        return Err(());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
impl IntoElement for WindowsWebView {
    type Element = WindowsWebViewElement;

    fn into_element(self) -> Self::Element {
        WindowsWebViewElement { webview: self }
    }
}

#[cfg(target_os = "windows")]
impl IntoElement for WindowsWebViewElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(target_os = "windows")]
pub(crate) struct WindowsWebViewElement {
    webview: WindowsWebView,
}

#[cfg(target_os = "windows")]
impl Element for WindowsWebViewElement {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout = window.request_layout(
            Style {
                size: Size::full(),
                flex_shrink: 1.,
                ..Default::default()
            },
            [],
            cx,
        );
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        self.webview.set_bounds(bounds, window.scale_factor());
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

#[cfg(target_os = "windows")]
struct WebHost(windows::Win32::Foundation::HWND);

#[cfg(target_os = "windows")]
impl raw_window_handle::HasWindowHandle for WebHost {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::{RawWindowHandle, Win32WindowHandle, WindowHandle};

        let hwnd = NonZeroIsize::new(self.0.0 as isize)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the worker created `self.0`, owns its message pump, and keeps
        // the HWND alive for the returned borrow.
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

#[cfg(target_os = "windows")]
fn run_windows_webview(
    parent: isize,
    rx: mpsc::Receiver<WorkerCommand>,
    ready: mpsc::SyncSender<Result<u32, String>>,
    events: smol::channel::Sender<WorkerEvent>,
) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW,
        TranslateMessage,
    };

    let parent = HWND(parent as *mut _);
    let host_hwnd = match create_web_host(parent) {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let host = WebHost(host_hwnd);
    let page_events = events.clone();
    let navigation_in_flight = Arc::new(AtomicBool::new(false));
    let page_navigation_in_flight = navigation_in_flight.clone();
    let builder = wry::WebViewBuilder::new().with_on_page_load_handler(move |event, _url| {
        if matches!(event, wry::PageLoadEvent::Finished)
            && page_navigation_in_flight.swap(false, Ordering::AcqRel)
        {
            let _ = page_events.try_send(WorkerEvent::PageLoaded);
        }
    });
    #[cfg(debug_assertions)]
    let builder = builder.with_devtools(true);
    // `lb-wry` calls `CoInitializeEx(..., COINIT_APARTMENTTHREADED)` from
    // `build`. Construction stays on this fresh worker so every later Wry call
    // runs in the same STA that owns WebView2 and the WebHost message queue.
    let webview = match builder.build(&host) {
        Ok(webview) => webview,
        Err(error) => {
            // SAFETY: no WebView was created and this worker owns the host.
            let _ = unsafe { DestroyWindow(host_hwnd) };
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    let mut message = MSG::default();
    // `PostThreadMessageW` needs an existing thread queue. Create it before the
    // ready signal so every subsequent command can wake the blocking pump.
    let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        drop(webview);
        let _ = unsafe { DestroyWindow(host_hwnd) };
        return;
    }

    loop {
        // Blocks until WebView2 or `wake_worker` posts a real thread message;
        // there is no periodic timeout waking an otherwise idle application.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        if message.message == WORKER_WAKE_MESSAGE {
            if !drain_worker_commands(&rx, host_hwnd, &webview, &navigation_in_flight) {
                break;
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    drop(webview);
    // SAFETY: this thread created the host and has finished using it.
    let _ = unsafe { DestroyWindow(host_hwnd) };
}

#[cfg(target_os = "windows")]
fn create_web_host(
    parent: windows::Win32::Foundation::HWND,
) -> Result<windows::Win32::Foundation::HWND, String> {
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_CHILD,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    };
    use windows::core::w;

    unsafe extern "system" fn web_host_proc(
        hwnd: windows::Win32::Foundation::HWND,
        message: u32,
        wparam: windows::Win32::Foundation::WPARAM,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        // SAFETY: this is the default procedure for the private child class.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    let module = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(web_host_proc),
        hInstance: instance,
        lpszClassName: w!("MarkTurboWebHost"),
        ..Default::default()
    };
    // A zero return also means the process already registered this class; both
    // cases are valid because every worker uses the same procedure.
    let _ = unsafe { RegisterClassW(&class) };
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("MarkTurboWebHost"),
            w!("WebHost"),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn drain_worker_commands(
    rx: &mpsc::Receiver<WorkerCommand>,
    host: windows::Win32::Foundation::HWND,
    webview: &wry::WebView,
    navigation_in_flight: &AtomicBool,
) -> bool {
    let mut latest_bounds = None;
    loop {
        match rx.try_recv() {
            Ok(WorkerCommand::Bounds(bounds)) => latest_bounds = Some(bounds),
            Ok(WorkerCommand::Shutdown) => return false,
            Ok(command) => {
                if !apply_worker_command(command, host, webview, navigation_in_flight) {
                    return false;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
    if let Some(bounds) = latest_bounds {
        apply_worker_command(
            WorkerCommand::Bounds(bounds),
            host,
            webview,
            navigation_in_flight,
        )
    } else {
        true
    }
}

#[cfg(target_os = "windows")]
fn apply_worker_command(
    command: WorkerCommand,
    host: windows::Win32::Foundation::HWND,
    webview: &wry::WebView,
    navigation_in_flight: &AtomicBool,
) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SetWindowPos, ShowWindow,
    };

    match command {
        WorkerCommand::Show => {
            let _ = unsafe { ShowWindow(host, SW_SHOW) };
        }
        WorkerCommand::Hide => {
            let _ = unsafe { ShowWindow(host, SW_HIDE) };
        }
        WorkerCommand::LoadUrl(url) => {
            navigation_in_flight.store(true, Ordering::Release);
            if let Err(error) = webview.load_url(&url) {
                navigation_in_flight.store(false, Ordering::Release);
                log::warn!("failed to load Web preview: {error}");
            }
        }
        WorkerCommand::Evaluate(script) => {
            if let Err(error) = webview.evaluate_script(&script) {
                log::debug!("failed to synchronize Web preview scroll: {error}");
            }
        }
        WorkerCommand::Bounds(bounds) => {
            let result = unsafe {
                SetWindowPos(
                    host,
                    Some(HWND_TOP),
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                )
            };
            if let Err(error) = result {
                log::debug!("failed to position Web preview: {error}");
            }
        }
        WorkerCommand::Shutdown => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    use super::{Navigation, PendingScroll, WebPayloadKey, WebSurface};

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn hide_focus_handoff_is_edge_triggered() {
        let mut surface = WebSurface {
            visible: true,
            ..Default::default()
        };

        assert!(surface.begin_hide());
        assert!(!surface.begin_hide());
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn unchanged_payload_avoids_an_html_clone() {
        let key = WebPayloadKey {
            tab: 3,
            revision: 9,
        };
        let surface = WebSurface {
            current: Some(key),
            visible: true,
            lent_tab: Some(key.tab),
            ..Default::default()
        };

        assert_eq!(surface.payload_update(key, "large html"), None);
        assert_eq!(surface.lent_tab, Some(3));
        assert!(surface.visible);
        assert_eq!(
            surface.payload_update(
                WebPayloadKey {
                    revision: 10,
                    ..key
                },
                "new html"
            ),
            Some("new html".into())
        );
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn old_page_finish_cannot_consume_the_latest_scroll() {
        let old = WebPayloadKey {
            tab: 1,
            revision: 4,
        };
        let new = WebPayloadKey {
            tab: 1,
            revision: 5,
        };
        let mut surface = WebSurface::default();
        let first = surface.begin_navigation(old);
        surface.pending_scroll = Some(PendingScroll {
            key: new,
            fraction: 0.75,
        });

        assert_eq!(surface.payload_update(new, "latest html"), None);
        assert_eq!(surface.finish_navigation(), Some(first));
        assert_eq!(surface.ready_scroll(old), None);
        assert_eq!(surface.ready_scroll(new), None);
        assert_eq!(surface.pending_scroll.unwrap().key, new);
        assert_eq!(
            surface.payload_update(new, "latest html"),
            Some("latest html".into())
        );

        let latest = surface.begin_navigation(new);
        assert_eq!(surface.ready_scroll(new), None);
        assert_eq!(surface.finish_navigation(), Some(latest));
        assert_eq!(surface.ready_scroll(new), Some(0.75));
        assert_eq!(surface.loading, None);
        assert_eq!(surface.loaded, Some(Navigation { key: new }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn trusted_urls_are_loaded_verbatim() {
        use super::WorkerCommand;

        let url = "file:///C:/docs/index.html#/settings".to_string();
        let command = WorkerCommand::LoadUrl(url.clone());
        let WorkerCommand::LoadUrl(queued) = command else {
            unreachable!()
        };
        assert_eq!(queued, url);

        let source = crate::views::production_source(include_str!("web_surface.rs"));
        assert!(!source.contains("markturbo-navigation="));
        assert!(source.contains("webview.load_url(&url)"));
        assert!(source.contains("page_navigation_in_flight.swap(false, Ordering::AcqRel)"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_disconnected_worker_channel_is_reported() {
        use super::{WindowsWebView, WorkerCommand, WorkerConnection};
        use std::sync::{Arc, Mutex, mpsc};

        let (tx, rx) = mpsc::channel();
        drop(rx);
        let webview = WindowsWebView(Arc::new(WorkerConnection {
            tx,
            last_bounds: Mutex::new(None),
            thread_id: 0,
        }));

        assert!(webview.send(WorkerCommand::Show).is_err());
    }

    #[test]
    fn a_disconnected_worker_is_cleared_reported_and_requeued() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source
            .find("fn webview_connection_lost")
            .expect("disconnect recovery");
        let body = &source[start..];
        let end = body
            .find("\n    #[cfg(target_os = \"windows\")]\n    fn start_windows_webview")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(body.contains("self.web.webview = None"));
        assert!(body.contains("self.web.current = None"));
        assert!(body.contains("self.lend_webview(None, None, cx)"));
        assert!(body.contains("self.set_status("));
        assert!(body.contains("self.web_dirty(cx)"));
    }

    #[test]
    fn render_does_not_touch_the_webview() {
        let source = crate::views::production_source(include_str!("../workspace.rs"));
        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Render impl")
            .1;
        let body = render.split("\n/// Keybindings").next().unwrap_or(render);

        for forbidden in ["sync_webview(", "WorkerCommand::", "create_webview("] {
            assert!(!body.contains(forbidden));
        }
        assert!(!source.contains("fn sync_webview"));
    }

    #[test]
    fn the_sync_is_deferred_coalesced_and_reached_fallibly() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source.find("fn mark_dirty").expect("mark_dirty must exist");
        let body = &source[start..];
        let end = body.find("\nimpl Workspace").unwrap_or(body.len());
        let body = &body[..end];

        assert!(body.contains("cx.defer("));
        assert!(body.contains("if self.sync_pending") && body.contains("self.sync_pending = true"));
        assert!(body.contains("cx.with_window(") && body.contains(".is_err()"));
    }

    #[test]
    fn the_webview_is_lent_to_exactly_one_tab() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source
            .find("fn lend_webview")
            .expect("lend_webview must exist");
        let body = &source[start..];
        let end = body.find("\n    }").unwrap_or(body.len());
        let body = &body[..end];

        assert!(body.contains("(Some(ix) == tab)"));
        assert!(body.contains("document_views()"));
        let sync = source.find("fn sync_webview").expect("sync_webview");
        let sync = &source[sync..start];
        assert!(sync.contains("if self.web.lent_tab != Some(key.tab)"));
    }

    #[test]
    fn windows_uses_one_worker_owned_child_webview() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));

        assert!(source.contains("thread::Builder::new()"));
        assert!(source.contains("COINIT_APARTMENTTHREADED"));
        assert!(source.contains("WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS"));
        assert!(source.contains("builder.build(&host)"));
        assert!(source.contains("WorkerCommand::Bounds"));
        assert!(source.contains("GetMessageW") && source.contains("PostThreadMessageW"));
        assert!(source.contains("let mut latest_bounds = None"));
        assert!(source.contains("with_on_page_load_handler"));
        assert!(source.contains("WorkerEvent::PageLoaded"));
        assert!(!source.contains("recv_timeout"));
        for forbidden in ["WS_OVERLAPPEDWINDOW", "SetForegroundWindow", "open_window("] {
            assert!(
                !source.contains(forbidden),
                "forbidden companion-window API: {forbidden}"
            );
        }
    }

    #[test]
    fn windows_publishes_only_a_ready_worker_and_never_joins_on_drop() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let drop_start = source
            .find("impl Drop for WorkerConnection")
            .expect("connection drop");
        let drop_body = &source[drop_start..];
        let drop_end = drop_body.find("\n}").unwrap_or(drop_body.len());
        let drop_body = &drop_body[..drop_end];

        assert!(source.contains("ready.send(Ok(thread_id))"));
        assert!(source.contains("background_spawn(async move { startup.wait() })"));
        assert!(source.contains("this.web.webview = Some(worker)"));
        assert!(!drop_body.contains("join("));
        assert!(drop_body.contains("WorkerCommand::Shutdown"));
    }

    #[test]
    fn hide_restores_focus_only_on_transition_and_navigation_waits_for_finish() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let sync = source.find("fn sync_webview").expect("sync_webview");
        let sync_body = &source[sync..];
        let sync_end = sync_body
            .find("\n    #[cfg(target_os = \"windows\")]\n    fn start_windows_webview")
            .unwrap_or(sync_body.len());
        let sync_body = &sync_body[..sync_end];
        let apply = source
            .find("fn apply_worker_command")
            .map(|start| &source[start..])
            .expect("worker command application");
        let native_focus = source
            .find("fn focus_native_window")
            .map(|start| &source[start..])
            .expect("native focus helper");
        let native_focus_end = native_focus
            .find("\n#[cfg(target_os = \"macos\")]\nfn hide_webview")
            .unwrap_or(native_focus.len());
        let native_focus = &native_focus[..native_focus_end];

        assert!(sync_body.contains("let was_visible = self.web.begin_hide()"));
        assert!(sync_body.contains("if was_visible"));
        assert_eq!(sync_body.matches("focus_native_window(window)").count(), 1);
        assert!(
            sync_body.find("focus_native_window(window)")
                < sync_body.find("hide_webview(webview, cx)")
        );
        assert!(sync_body.contains("window.focus(&self.focus_handle, cx)"));
        assert!(sync_body.contains("self.web.begin_navigation(key)"));
        assert!(sync_body.contains("self.web.ready_scroll(key)"));
        assert!(native_focus.contains("SetFocus(Some(hwnd))"));
        assert!(native_focus.contains("GetFocus() == hwnd"));
        assert!(!apply.contains("focus_parent()") && !apply.contains("SetFocus("));
    }

    #[test]
    fn macos_navigation_completion_is_event_driven() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let sync = source.find("fn sync_webview").expect("sync_webview");
        let sync_body = &source[sync..];
        let sync_end = sync_body
            .find("\n    #[cfg(any(target_os = \"windows\", target_os = \"macos\"))]\n    fn webview_connection_lost")
            .expect("connection-loss handler");
        let sync_body = &sync_body[..sync_end];
        let create = source
            .find("fn create_webview")
            .map(|start| &source[start..])
            .expect("macOS WebView creation");
        let create_end = create
            .find("\n#[cfg(target_os = \"windows\")]\nfn hide_webview")
            .expect("Windows hide helper");
        let create = &create[..create_end];

        assert!(sync_body.contains("self.web.navigation_in_flight.set(true)"));
        assert!(!sync_body.contains("self.web.finish_navigation()"));
        assert!(create.contains("with_on_page_load_handler"));
        assert!(create.contains("wry::PageLoadEvent::Finished"));
        assert!(create.contains("navigation_in_flight.replace(false)"));
        assert!(create.contains("page_loaded.try_send(())"));
    }

    #[test]
    fn windows_prepaint_only_queues_bounds() {
        let source = crate::views::production_source(include_str!("web_surface.rs"));
        let start = source
            .find("fn prepaint(")
            .expect("the Windows element prepaint");
        let body = &source[start..];
        let end = body.find("\n    fn paint(").expect("the paint method");
        let body = &body[..end];

        assert!(body.contains("self.webview.set_bounds(bounds, window.scale_factor())"));
        assert!(!body.contains("wry::") && !body.contains("evaluate_script"));
    }

    #[test]
    fn companion_preview_ui_cannot_return() {
        let i18n = crate::views::production_source(include_str!("../../i18n.rs"));
        let document = crate::views::production_source(include_str!("../document.rs"));
        let surface = crate::views::production_source(include_str!("web_surface.rs"));

        for forbidden in ["WebPreviewWindow", "ShowWebPreview", "companion window"] {
            assert!(!i18n.contains(forbidden));
            assert!(!document.contains(forbidden));
            assert!(!surface.contains(forbidden));
        }
    }
}
