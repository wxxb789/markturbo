//! The window's single Web preview surface.
//!
//! One WebView is shared by every tab. On macOS it stays in GPUI's existing
//! `gpui-wry` element path. On Windows it belongs to a dedicated STA worker:
//! WebView2 pumps messages while it is mutated, and doing that on GPUI's thread
//! can re-enter `AppCell` while a draw already holds the mutable borrow.

use gpui_kit::*;

use super::{DocumentView, Workspace};
use mt_core::document::lifecycle::DocumentId;

#[cfg(target_os = "macos")]
use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::num::NonZeroIsize;
#[cfg(target_os = "macos")]
use std::rc::Rc;
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex, mpsc};
#[cfg(target_os = "windows")]
use std::thread;

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DocumentLease {
    document_id: DocumentId,
    tab: usize,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WebPayloadKey {
    document_id: DocumentId,
    tab: usize,
    revision: u64,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl WebPayloadKey {
    fn lease(self) -> DocumentLease {
        DocumentLease {
            document_id: self.document_id,
            tab: self.tab,
        }
    }
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
    #[cfg(target_os = "macos")]
    webview_instance: Option<u64>,
    #[cfg(target_os = "macos")]
    next_webview_instance: u64,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    sync_pending: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    current: Option<WebPayloadKey>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pending_scroll: Option<PendingScroll>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    visible: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    lent_document: Option<DocumentLease>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    loading: Option<Navigation>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    loaded: Option<Navigation>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    failed: Option<WebPayloadKey>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    retrying: Option<WebPayloadKey>,
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
        let replacing_document = self.requires_surface_replacement(key);
        (self.current != Some(key) && (self.loading.is_none() || replacing_document))
            .then(|| html.to_string())
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn requires_surface_replacement(&self, key: WebPayloadKey) -> bool {
        [
            self.current,
            self.loading.map(|navigation| navigation.key),
            self.loaded.map(|navigation| navigation.key),
        ]
        .into_iter()
        .flatten()
        .any(|current| current.lease() != key.lease())
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn abandon_for_document_change(&mut self, key: WebPayloadKey) -> Option<DocumentLease> {
        let lent = self.lent_document.take();
        self.current = None;
        self.loading = None;
        self.loaded = None;
        self.visible = false;
        if self
            .pending_scroll
            .is_some_and(|pending| pending.key.lease() != key.lease())
        {
            self.pending_scroll = None;
        }
        if self
            .retrying
            .is_some_and(|retrying| retrying.lease() != key.lease())
        {
            self.retrying = None;
        }
        #[cfg(target_os = "macos")]
        {
            self.webview_instance = None;
        }
        lent
    }

    #[cfg(target_os = "macos")]
    fn next_webview_instance(&mut self) -> u64 {
        self.next_webview_instance = self.next_webview_instance.wrapping_add(1);
        self.webview_instance = Some(self.next_webview_instance);
        self.next_webview_instance
    }

    #[cfg(target_os = "macos")]
    fn accepts_page_event(&self, instance: u64) -> bool {
        self.webview_instance == Some(instance) && self.loading.is_some()
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
        if self.retrying == Some(navigation.key) {
            self.retrying = None;
        }
        Some(navigation)
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn is_loading(&self, navigation: Navigation) -> bool {
        self.loading == Some(navigation)
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn finish_navigation_for(&mut self, navigation: Navigation) -> Option<Navigation> {
        if self.is_loading(navigation) {
            self.finish_navigation()
        } else {
            None
        }
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

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn should_retry(&mut self, key: WebPayloadKey) -> bool {
        if self.retrying.is_some_and(|retrying| retrying != key) {
            self.retrying = None;
        }
        match self.failed {
            Some(failed) if failed == key => false,
            Some(_) => {
                self.failed = None;
                true
            }
            None => true,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn retry_failed(&mut self, key: WebPayloadKey) -> bool {
        if !self.failed.is_some_and(|failed| {
            failed.document_id == key.document_id && failed.revision == key.revision
        }) {
            return false;
        }
        self.failed = None;
        self.retrying = Some(key);
        true
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn clear_failure_for_document(&mut self, document_id: DocumentId) {
        if self
            .failed
            .is_some_and(|failed| failed.document_id == document_id)
        {
            self.failed = None;
        }
        if self
            .retrying
            .is_some_and(|retrying| retrying.document_id == document_id)
        {
            self.retrying = None;
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn fail_operation(&mut self, key: WebPayloadKey) -> bool {
        self.webview = None;
        self.current = None;
        self.pending_scroll = None;
        self.visible = false;
        self.loading = None;
        self.loaded = None;
        self.failed = Some(key);
        self.retrying = None;
        self.lent_document.take().is_some()
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
                key: WebPayloadKey {
                    document_id: document.id(),
                    tab,
                    revision,
                },
                fraction,
            });
            self.web_dirty(cx);
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = (fraction, cx);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_intent(&mut self, cx: &App) -> WebIntent {
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
        if doc.has_web_preview_failure() {
            return WebIntent::Hide;
        }
        match doc.web_payload() {
            Some((html, revision)) => {
                let key = WebPayloadKey {
                    document_id: doc.id(),
                    tab,
                    revision,
                };
                if !self.web.should_retry(key) {
                    return WebIntent::Unchanged;
                }
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
                    window.focus(&self.focus_handle, cx);
                    let failed_key = self
                        .web
                        .loading
                        .map(|navigation| navigation.key)
                        .or(self.web.current);
                    if let Some(key) = failed_key {
                        self.webview_operation_failed(key, window, cx);
                        return;
                    }
                }
                if self.web.lent_document.take().is_some() {
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

        if self.web.requires_surface_replacement(key) {
            let was_visible = self.web.begin_hide();
            #[cfg(target_os = "windows")]
            if was_visible && !focus_native_window(window) {
                log::debug!("failed to restore native focus before replacing Web preview");
            }
            if let Some(webview) = &self.web.webview {
                let _ = hide_webview(webview, cx);
            }
            let previous_lease = self.web.abandon_for_document_change(key);
            if previous_lease.is_some() {
                self.lend_webview(None, None, cx);
            }
            self.web.webview = None;
            #[cfg(target_os = "macos")]
            {
                self.web.navigation_in_flight = Rc::new(Cell::new(false));
            }
            if was_visible {
                window.focus(&self.focus_handle, cx);
            }
        }

        let webview = match &self.web.webview {
            Some(webview) => webview.clone(),
            None => {
                #[cfg(target_os = "windows")]
                {
                    self.start_windows_webview(key, window, cx);
                    return;
                }
                #[cfg(target_os = "macos")]
                {
                    let navigation_in_flight = Rc::new(Cell::new(false));
                    let instance = self.web.next_webview_instance();
                    let (page_loaded, page_events) = smol::channel::unbounded();
                    let webview = match create_webview(
                        window,
                        navigation_in_flight.clone(),
                        page_loaded,
                        instance,
                        cx,
                    ) {
                        Ok(webview) => webview,
                        Err(_) => {
                            self.webview_operation_failed(key, window, cx);
                            return;
                        }
                    };
                    let _ = hide_webview(&webview, cx);
                    self.web.navigation_in_flight = navigation_in_flight;
                    self.web.webview = Some(webview.clone());
                    let this = cx.entity().downgrade();
                    cx.spawn(async move |_, cx| {
                        while let Ok(event) = page_events.recv().await {
                            crate::views::try_update(&this, cx, |this, cx| {
                                this.mac_webview_page_loaded(event.instance, cx);
                            });
                        }
                    })
                    .detach();
                    webview
                }
            }
        };

        if self.web.lent_document != Some(key.lease()) {
            self.lend_webview(Some(key.lease()), Some(webview.clone()), cx);
            self.web.lent_document = Some(key.lease());
        }

        if let Some(html) = html {
            let navigation = self.web.begin_navigation(key);
            #[cfg(target_os = "macos")]
            self.web.navigation_in_flight.set(true);
            if !load_webview(&webview, crate::web::to_data_url(&html), navigation, cx) {
                self.webview_operation_failed(key, window, cx);
                return;
            }
        }

        let safe_to_show = self
            .web
            .loaded
            .is_some_and(|navigation| navigation.key.lease() == key.lease());
        if safe_to_show && !self.web.visible {
            if !show_webview(&webview, cx) {
                self.webview_operation_failed(key, window, cx);
                return;
            }
            self.web.visible = true;
        }

        if let Some(fraction) = self.web.ready_scroll(key) {
            if !evaluate_webview(&webview, scroll_script(fraction), cx) {
                self.webview_operation_failed(key, window, cx);
                return;
            }
            self.web.pending_scroll = None;
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_operation_failed(
        &mut self,
        key: WebPayloadKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = self.web.begin_hide();
        #[cfg(target_os = "windows")]
        if was_visible && !focus_native_window(window) {
            log::debug!("failed to restore native focus after Web preview failure");
        }
        if let Some(webview) = &self.web.webview {
            let _ = hide_webview(webview, cx);
        }
        let was_lent = self.web.fail_operation(key);
        #[cfg(target_os = "macos")]
        self.web.navigation_in_flight.set(false);
        #[cfg(target_os = "macos")]
        {
            self.web.webview_instance = None;
        }
        if was_lent {
            self.lend_webview(None, None, cx);
        }
        if was_visible {
            window.focus(&self.focus_handle, cx);
        }
        self.mark_web_preview_failed(key, cx);
    }

    #[cfg(target_os = "windows")]
    fn handle_webview_event(
        &mut self,
        worker_id: usize,
        event: WorkerEvent,
        window: &mut Window,
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
            WorkerEvent::PageLoaded(navigation) => {
                if let Some(navigation) = self.web.finish_navigation_for(navigation) {
                    self.clear_web_preview_failure(navigation.key, cx);
                    self.web_dirty(cx);
                }
            }
            WorkerEvent::NavigationFailed(navigation) => {
                if self.web.is_loading(navigation) {
                    self.webview_operation_failed(navigation.key, window, cx);
                    self.web_dirty(cx);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn mac_webview_page_loaded(&mut self, instance: u64, cx: &mut Context<Self>) {
        if self.web.accepts_page_event(instance)
            && let Some(navigation) = self.web.finish_navigation()
        {
            self.clear_web_preview_failure(navigation.key, cx);
            self.web_dirty(cx);
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn mark_web_preview_failed(&mut self, key: WebPayloadKey, cx: &mut Context<Self>) {
        if let Some(document) = self
            .document_views()
            .into_iter()
            .find(|document| document.read(cx).id() == key.document_id)
        {
            document.update(cx, |document, cx| {
                document.mark_web_preview_failed(key.revision, cx);
            });
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn clear_web_preview_failure(&mut self, key: WebPayloadKey, cx: &mut Context<Self>) {
        if let Some(document) = self
            .document_views()
            .into_iter()
            .find(|document| document.read(cx).id() == key.document_id)
        {
            document.update(cx, |document, cx| {
                document.clear_web_preview_failure(key.revision, cx);
            });
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(super) fn retry_web_preview(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &mut Context<Self>,
    ) {
        let Some(active_document) = self.active_document() else {
            return;
        };
        let document_id = document.read(cx).id();
        if active_document.read(cx).id() != document_id {
            return;
        }
        let active_tab = self.tabs.active_index();
        let revision = {
            let active = document.read(cx);
            if !active.layout().uses_webview() {
                return;
            }
            active.web_payload().map(|(_, revision)| revision)
        };
        if let Some(revision) = revision {
            self.web.retry_failed(WebPayloadKey {
                document_id,
                tab: active_tab,
                revision,
            });
            self.web_dirty(cx);
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(super) fn web_preview_layout_left(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &Context<Self>,
    ) {
        self.web.clear_failure_for_document(document.read(cx).id());
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub(super) fn retry_web_preview(&mut self, _: &Entity<DocumentView>, _: &mut Context<Self>) {}

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub(super) fn web_preview_layout_left(&mut self, _: &Entity<DocumentView>, _: &Context<Self>) {}

    #[cfg(target_os = "windows")]
    fn start_windows_webview(
        &mut self,
        key: WebPayloadKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.web.starting {
            return;
        }
        let startup = match WindowsWebView::start(window) {
            Ok(startup) => startup,
            Err(_) => {
                self.webview_operation_failed(key, window, cx);
                return;
            }
        };
        self.web.starting = true;
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx.background_spawn(async move { startup.wait() }).await;
            let ready = match result {
                Ok(ready) => ready,
                Err(_) => {
                    crate::views::try_update_in(&this, cx, |this, window, cx| {
                        this.web.starting = false;
                        this.webview_operation_failed(key, window, cx);
                        this.web_dirty(cx);
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
                crate::views::try_update_in(&this, cx, |this, window, cx| {
                    this.handle_webview_event(worker_id, event, window, cx);
                });
            }
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                let owns_worker = this
                    .web
                    .webview
                    .as_ref()
                    .is_some_and(|worker| worker.identity() == worker_id);
                if owns_worker {
                    let failed_key = this
                        .web
                        .loading
                        .map(|navigation| navigation.key)
                        .or(this.web.current)
                        .unwrap_or(key);
                    this.webview_operation_failed(failed_key, window, cx);
                    this.web_dirty(cx);
                }
            });
        })
        .detach();
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn lend_webview(
        &mut self,
        lease: Option<DocumentLease>,
        webview: Option<PlatformWebView>,
        cx: &mut Context<Self>,
    ) {
        for (ix, doc) in self.document_views().into_iter().enumerate() {
            let owns_lease = lease
                .is_some_and(|lease| lease.tab == ix && lease.document_id == doc.read(cx).id());
            let lent = owns_lease.then(|| webview.clone()).flatten();
            doc.update(cx, |doc, cx| doc.set_webview(lent, cx));
        }
    }
}

#[cfg(target_os = "windows")]
type PlatformWebView = WindowsWebView;
#[cfg(target_os = "macos")]
type PlatformWebView = Entity<gpui_wry::WebView>;

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacPageLoaded {
    instance: u64,
}

#[cfg(target_os = "macos")]
fn create_webview(
    window: &mut Window,
    navigation_in_flight: Rc<Cell<bool>>,
    page_loaded: smol::channel::Sender<MacPageLoaded>,
    instance: u64,
    cx: &mut App,
) -> Result<Entity<gpui_wry::WebView>, MacWebViewCreateError> {
    use raw_window_handle::HasWindowHandle as _;

    let handle = window
        .window_handle()
        .map_err(|_| MacWebViewCreateError::WindowHandle)?;
    let builder = wry::WebViewBuilder::new().with_on_page_load_handler(move |event, _url| {
        if matches!(event, wry::PageLoadEvent::Finished) && navigation_in_flight.replace(false) {
            let _ = page_loaded.try_send(MacPageLoaded { instance });
        }
    });
    #[cfg(debug_assertions)]
    let builder = builder.with_devtools(true);
    let webview = builder
        .build_as_child(&handle)
        .map_err(|_| MacWebViewCreateError::Build)?;
    Ok(cx.new(|cx| gpui_wry::WebView::new(webview, window, cx)))
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacWebViewCreateError {
    WindowHandle,
    Build,
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
    webview.update(cx, |webview, cx| {
        webview.show();
        cx.notify();
    });
    true
}

#[cfg(target_os = "windows")]
fn load_webview(
    webview: &WindowsWebView,
    url: String,
    navigation: Navigation,
    _: &mut App,
) -> bool {
    webview
        .send(WorkerCommand::LoadUrl { url, navigation })
        .is_ok()
}

#[cfg(target_os = "macos")]
fn load_webview(
    webview: &Entity<gpui_wry::WebView>,
    url: String,
    _: Navigation,
    cx: &mut App,
) -> bool {
    webview.update(cx, |webview, _| webview.raw().load_url(&url).is_ok())
}

#[cfg(target_os = "windows")]
fn evaluate_webview(webview: &WindowsWebView, script: String, _: &mut App) -> bool {
    webview.send(WorkerCommand::Evaluate(script)).is_ok()
}

#[cfg(target_os = "macos")]
fn evaluate_webview(webview: &Entity<gpui_wry::WebView>, script: String, cx: &mut App) -> bool {
    webview.update(cx, |webview, _| {
        webview.raw().evaluate_script(&script).is_ok()
    })
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
/// Keep independently rounded GPUI bounds inside the actual HWND client area.
///
/// At 125% DPI, the runtime probe measured a 1,382 px child in a 1,381 px
/// client. Clamping here uses the OS boundary that `SetWindowPos` must honor.
fn clamp_bounds_to_client(
    bounds: PhysicalBounds,
    client_width: i32,
    client_height: i32,
) -> PhysicalBounds {
    let client_width = client_width.max(0);
    let client_height = client_height.max(0);
    let x = bounds.x.clamp(0, client_width);
    let y = bounds.y.clamp(0, client_height);
    let right = bounds
        .x
        .saturating_add(bounds.width.max(0))
        .clamp(x, client_width);
    let bottom = bounds
        .y
        .saturating_add(bounds.height.max(0))
        .clamp(y, client_height);
    PhysicalBounds {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

#[cfg(target_os = "windows")]
enum WorkerCommand {
    Show,
    Hide,
    LoadUrl { url: String, navigation: Navigation },
    Evaluate(String),
    Bounds(PhysicalBounds),
    Shutdown,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerEvent {
    PageLoaded(Navigation),
    NavigationFailed(Navigation),
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

    fn start(window: &Window) -> Result<WindowsWebViewStartup, String> {
        use raw_window_handle::RawWindowHandle;

        let handle = raw_window_handle::HasWindowHandle::window_handle(window)
            .map_err(|_| "the application window handle is unavailable".to_string())?;
        let parent = match handle.as_raw() {
            RawWindowHandle::Win32(handle) => handle.hwnd.get(),
            _ => return Err("the application window is not a Win32 window".into()),
        };
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (event_tx, events) = smol::channel::unbounded();
        let worker = thread::Builder::new()
            .name("markturbo-webview-sta".into())
            .spawn(move || run_windows_webview(parent, rx, ready_tx, event_tx))
            .map_err(|_| "the Web preview worker could not be started".to_string())?;
        Ok(WindowsWebViewStartup {
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

    let Some(data_dir) = mt_core::runtime_paths::webview_data_dir() else {
        let _ = ready.send(Err(
            "cannot resolve the MarkTurbo WebView data directory".to_string()
        ));
        return;
    };
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        let _ = ready.send(Err(format!(
            "cannot create WebView data directory {}: {error}",
            data_dir.display()
        )));
        return;
    }
    let mut web_context = wry::WebContext::new(Some(data_dir));

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
    let navigation_in_flight = Arc::new(Mutex::new(None::<Navigation>));
    let page_navigation_in_flight = navigation_in_flight.clone();
    let builder = wry::WebViewBuilder::new_with_web_context(&mut web_context)
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, wry::PageLoadEvent::Finished)
                && let Some(navigation) = page_navigation_in_flight
                    .lock()
                    .ok()
                    .and_then(|mut navigation| navigation.take())
            {
                let _ = page_events.try_send(WorkerEvent::PageLoaded(navigation));
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
            if !drain_worker_commands(
                &rx,
                parent,
                host_hwnd,
                &webview,
                &navigation_in_flight,
                &events,
            ) {
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
    parent: windows::Win32::Foundation::HWND,
    host: windows::Win32::Foundation::HWND,
    webview: &wry::WebView,
    navigation_in_flight: &Mutex<Option<Navigation>>,
    events: &smol::channel::Sender<WorkerEvent>,
) -> bool {
    let mut latest_bounds = None;
    loop {
        match rx.try_recv() {
            Ok(WorkerCommand::Bounds(bounds)) => latest_bounds = Some(bounds),
            Ok(WorkerCommand::Shutdown) => return false,
            Ok(command) => {
                if !apply_worker_command(
                    command,
                    parent,
                    host,
                    webview,
                    navigation_in_flight,
                    events,
                ) {
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
            parent,
            host,
            webview,
            navigation_in_flight,
            events,
        )
    } else {
        true
    }
}

#[cfg(target_os = "windows")]
fn apply_worker_command(
    command: WorkerCommand,
    parent: windows::Win32::Foundation::HWND,
    host: windows::Win32::Foundation::HWND,
    webview: &wry::WebView,
    navigation_in_flight: &Mutex<Option<Navigation>>,
    events: &smol::channel::Sender<WorkerEvent>,
) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HWND_TOP, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SetWindowPos,
        ShowWindow,
    };

    match command {
        WorkerCommand::Show => {
            let _ = unsafe { ShowWindow(host, SW_SHOW) };
        }
        WorkerCommand::Hide => {
            let _ = unsafe { ShowWindow(host, SW_HIDE) };
        }
        WorkerCommand::LoadUrl { url, navigation } => {
            if let Ok(mut current) = navigation_in_flight.lock() {
                *current = Some(navigation);
            }
            if webview.load_url(&url).is_err() {
                if let Ok(mut current) = navigation_in_flight.lock()
                    && *current == Some(navigation)
                {
                    *current = None;
                }
                let _ = events.try_send(WorkerEvent::NavigationFailed(navigation));
            }
        }
        WorkerCommand::Evaluate(script) => {
            if let Err(error) = webview.evaluate_script(&script) {
                log::debug!("failed to synchronize Web preview scroll: {error}");
            }
        }
        WorkerCommand::Bounds(bounds) => {
            let mut client = RECT::default();
            let bounds = match unsafe { GetClientRect(parent, &mut client) } {
                Ok(()) => clamp_bounds_to_client(
                    bounds,
                    client.right - client.left,
                    client.bottom - client.top,
                ),
                Err(error) => {
                    log::debug!("failed to read Web preview parent bounds: {error}");
                    bounds
                }
            };
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
    #[cfg(target_os = "windows")]
    use super::{PhysicalBounds, clamp_bounds_to_client};
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    use mt_core::document::lifecycle::DocumentId;

    #[cfg(target_os = "windows")]
    #[test]
    fn child_bounds_are_clamped_to_parent_client() {
        let observed = PhysicalBounds {
            x: 0,
            y: 89,
            width: 1_382,
            height: 663,
        };

        assert_eq!(
            clamp_bounds_to_client(observed, 1_381, 777),
            PhysicalBounds {
                x: 0,
                y: 89,
                width: 1_381,
                height: 663,
            }
        );
        assert_eq!(
            clamp_bounds_to_client(
                PhysicalBounds {
                    x: -5,
                    y: -7,
                    width: 20,
                    height: 30,
                },
                10,
                10,
            ),
            PhysicalBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            }
        );
    }

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
            document_id: DocumentId::next(),
            tab: 3,
            revision: 9,
        };
        let surface = WebSurface {
            current: Some(key),
            visible: true,
            lent_document: Some(key.lease()),
            ..Default::default()
        };

        assert_eq!(surface.payload_update(key, "large html"), None);
        assert_eq!(surface.lent_document, Some(key.lease()));
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
        let document_id = DocumentId::next();
        let old = WebPayloadKey {
            document_id,
            tab: 1,
            revision: 4,
        };
        let new = WebPayloadKey {
            document_id,
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

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn failed_operation_stays_suppressed_until_a_real_retry_boundary() {
        let key = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 2,
            revision: 8,
        };
        let loaded = Navigation {
            key: WebPayloadKey { revision: 7, ..key },
        };
        let mut surface = WebSurface {
            current: Some(key),
            visible: true,
            lent_document: Some(key.lease()),
            loading: Some(Navigation { key }),
            loaded: Some(loaded),
            pending_scroll: Some(PendingScroll { key, fraction: 0.5 }),
            ..Default::default()
        };

        assert!(surface.fail_operation(key));
        assert_eq!(surface.current, None);
        assert!(!surface.visible);
        assert_eq!(surface.lent_document, None);
        assert_eq!(surface.loading, None);
        assert_eq!(surface.loaded, None);
        assert_eq!(surface.pending_scroll, None);
        assert!(!surface.should_retry(key));

        surface.begin_hide();
        assert!(!surface.should_retry(key));
        surface.clear_failure_for_document(key.document_id);
        assert!(surface.should_retry(key));
        assert_eq!(surface.failed, None);

        surface.fail_operation(key);

        let changed = WebPayloadKey { revision: 9, ..key };
        assert!(surface.should_retry(changed));
        assert_eq!(surface.failed, None);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn different_document_at_same_tab_and_revision_is_not_suppressed() {
        let failed_key = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 2,
            revision: 8,
        };
        let replacement_key = WebPayloadKey {
            document_id: DocumentId::next(),
            ..failed_key
        };
        let mut surface = WebSurface {
            failed: Some(failed_key),
            ..Default::default()
        };

        assert!(surface.should_retry(replacement_key));
        assert_eq!(surface.failed, None);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn explicit_retry_clears_failure_and_records_the_requested_payload() {
        let key = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 2,
            revision: 8,
        };
        let mut surface = WebSurface {
            failed: Some(key),
            ..Default::default()
        };

        assert!(surface.retry_failed(key));
        assert_eq!(surface.failed, None);
        assert_eq!(surface.retrying, Some(key));
        assert!(surface.should_retry(key));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn same_index_document_replacement_transfers_the_native_lease() {
        let old = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 2,
            revision: 4,
        };
        let replacement = WebPayloadKey {
            document_id: DocumentId::next(),
            ..old
        };
        let mut surface = WebSurface {
            current: Some(old),
            loaded: Some(Navigation { key: old }),
            visible: true,
            lent_document: Some(old.lease()),
            ..Default::default()
        };

        assert_eq!(old.tab, replacement.tab);
        assert_ne!(old.lease(), replacement.lease());
        assert!(surface.requires_surface_replacement(replacement));
        assert_eq!(
            surface.payload_update(replacement, "replacement html"),
            Some("replacement html".into())
        );
        assert_eq!(
            surface.abandon_for_document_change(replacement),
            Some(old.lease())
        );
        assert_eq!(surface.lent_document, None);
        assert_eq!(surface.loaded, None);
        assert!(!surface.visible);

        surface.lent_document = Some(replacement.lease());
        surface.begin_navigation(replacement);
        assert_eq!(surface.loading, Some(Navigation { key: replacement }));
        assert_eq!(surface.lent_document, Some(replacement.lease()));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn failed_document_is_unlent_and_another_tab_can_navigate() {
        let failed = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 1,
            revision: 7,
        };
        let other_tab = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 2,
            revision: 1,
        };
        let mut surface = WebSurface {
            current: Some(failed),
            loading: Some(Navigation { key: failed }),
            visible: true,
            lent_document: Some(failed.lease()),
            ..Default::default()
        };

        assert!(surface.fail_operation(failed));
        assert_eq!(surface.failed, Some(failed));
        assert_eq!(surface.current, None);
        assert_eq!(surface.loading, None);
        assert_eq!(surface.lent_document, None);
        assert!(!surface.visible);
        assert!(!surface.should_retry(failed));

        assert!(surface.should_retry(other_tab));
        surface.begin_navigation(other_tab);
        assert_eq!(surface.loading, Some(Navigation { key: other_tab }));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn replacing_a_loading_document_does_not_wait_for_its_page_event() {
        let old = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 3,
            revision: 5,
        };
        let replacement = WebPayloadKey {
            document_id: DocumentId::next(),
            ..old
        };
        let mut surface = WebSurface {
            lent_document: Some(old.lease()),
            ..Default::default()
        };
        surface.begin_navigation(old);

        assert!(surface.requires_surface_replacement(replacement));
        assert_eq!(
            surface.payload_update(replacement, "new document html"),
            Some("new document html".into())
        );
        assert_eq!(
            surface.abandon_for_document_change(replacement),
            Some(old.lease())
        );
        surface.lent_document = Some(replacement.lease());
        surface.begin_navigation(replacement);

        assert_eq!(
            surface.finish_navigation(),
            Some(Navigation { key: replacement })
        );
        assert_eq!(surface.loaded, Some(Navigation { key: replacement }));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn stale_worker_navigation_event_cannot_finish_a_replacement_attempt() {
        let old = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 3,
            revision: 5,
        };
        let replacement = WebPayloadKey {
            document_id: DocumentId::next(),
            ..old
        };
        let old_navigation = Navigation { key: old };
        let new_navigation = Navigation { key: replacement };
        let mut surface = WebSurface::default();
        surface.begin_navigation(old);
        surface.abandon_for_document_change(replacement);
        surface.begin_navigation(replacement);

        assert!(!surface.is_loading(old_navigation));
        assert_eq!(surface.finish_navigation_for(old_navigation), None);
        assert_eq!(surface.loading, Some(new_navigation));
        assert!(surface.is_loading(new_navigation));
        assert_eq!(
            surface.finish_navigation_for(new_navigation),
            Some(new_navigation)
        );
        assert_eq!(surface.loaded, Some(new_navigation));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_mac_page_completion_is_rejected_after_document_replacement() {
        let old = WebPayloadKey {
            document_id: DocumentId::next(),
            tab: 3,
            revision: 5,
        };
        let replacement = WebPayloadKey {
            document_id: DocumentId::next(),
            ..old
        };
        let mut surface = WebSurface::default();
        let old_instance = surface.next_webview_instance();
        surface.begin_navigation(old);
        let _ = surface.abandon_for_document_change(replacement);
        let new_instance = surface.next_webview_instance();
        surface.begin_navigation(replacement);

        assert!(!surface.accepts_page_event(old_instance));
        assert!(surface.accepts_page_event(new_instance));
        assert_eq!(surface.loading, Some(Navigation { key: replacement }));
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
}
