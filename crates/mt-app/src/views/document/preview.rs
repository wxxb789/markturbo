//! Derived native and Web preview state for one document view.
//!
//! This owns preview caches and their transitions, not editor identity or
//! source text. Callers feed it the currently authoritative parsed `Document`
//! and source/trust inputs when a preview refresh is warranted.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::text::{
    MarkdownExtensions, MarkdownNode, TextViewState, TextViewStyle, markdown_ast,
};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mt_core::rendering::RendererRegistry;
use mt_core::{DocType, Document};

use crate::web::{self, Trust};

use super::DocumentView;

/// Documents above this size skip live Web refresh while typing. The native
/// preview remains live; pausing the Web path avoids rendering a megabyte of
/// Markdown on a background task every debounce interval.
const LIVE_PREVIEW_LIMIT: usize = 512 * 1024;

/// Cached rendering state derived from a document view's current parsed source.
///
/// The owner holds neither an editor snapshot nor its identity/generation:
/// `DocumentView` remains authoritative and decides when a parse or source
/// transition is current before asking this cache to refresh.
pub(super) struct DocumentPreview {
    native: Entity<TextViewState>,
    /// Kept at one revision for the lifetime of the preview. Upstream gives
    /// each extension builder a global revision, and its `TextViewState`
    /// reparses only when that revision changes. Cloning preserves the same
    /// revision; rebuilding per frame would reparse the document every draw.
    /// Above its 4 KiB sync threshold, the async reparse notifies and would
    /// sustain an unbounded loop.
    extensions: MarkdownExtensions,
    registry: Arc<RendererRegistry>,
    web_html: Option<String>,
    web_revision: u64,
    web_failure_revision: Option<u64>,
    synced_row: Option<usize>,
}

impl DocumentPreview {
    pub(super) fn new(
        text: &str,
        registry: Arc<RendererRegistry>,
        cx: &mut Context<DocumentView>,
    ) -> Self {
        let native = cx.new(|cx| TextViewState::markdown(text, cx).selectable(true));
        let extensions = diagram_extensions(registry.clone());
        Self {
            native,
            extensions,
            registry,
            web_html: None,
            web_revision: 0,
            web_failure_revision: None,
            synced_row: None,
        }
    }

    pub(super) fn native_state(&self) -> &Entity<TextViewState> {
        &self.native
    }

    pub(super) fn native_extensions(&self) -> &MarkdownExtensions {
        &self.extensions
    }

    pub(super) fn set_native_text(&mut self, text: &str, cx: &mut Context<DocumentView>) {
        self.native.update(cx, |state, cx| state.set_text(text, cx));
    }

    pub(super) fn mark_synced_row(&mut self, row: usize) -> bool {
        if self.synced_row == Some(row) {
            return false;
        }
        self.synced_row = Some(row);
        true
    }

    pub(super) fn reset_synced_row(&mut self) {
        self.synced_row = None;
    }

    /// Refresh a visible Web cache, or discard a hidden cache so it cannot be
    /// reused after the parsed document changes.
    pub(super) fn refresh_web(
        &mut self,
        document: &Document,
        source_path: Option<&Path>,
        trust: Trust,
        visible: bool,
        cx: &App,
    ) {
        if visible {
            self.rebuild_web(document, source_path, trust, cx);
        } else if self.web_html.take().is_some() {
            self.web_revision = self.web_revision.wrapping_add(1);
        }
    }

    /// Build the first payload when a layout enters a Web preview.
    pub(super) fn ensure_web(
        &mut self,
        document: &Document,
        source_path: Option<&Path>,
        trust: Trust,
        cx: &App,
    ) {
        if self.web_html.is_none() {
            self.rebuild_web(document, source_path, trust, cx);
        }
    }

    /// Trust changes always replace the current payload, including revocation.
    pub(super) fn trust_changed(
        &mut self,
        document: &Document,
        source_path: Option<&Path>,
        trust: Trust,
        cx: &App,
    ) {
        self.rebuild_web(document, source_path, trust, cx);
    }

    /// Theme changes only rebuild a payload that has already been requested.
    pub(super) fn theme_changed(
        &mut self,
        document: &Document,
        source_path: Option<&Path>,
        trust: Trust,
        cx: &App,
    ) {
        if self.web_html.is_some() {
            self.rebuild_web(document, source_path, trust, cx);
        }
    }

    fn rebuild_web(
        &mut self,
        document: &Document,
        source_path: Option<&Path>,
        trust: Trust,
        cx: &App,
    ) {
        self.web_revision = self.web_revision.wrapping_add(1);
        if document.source().len() > LIVE_PREVIEW_LIMIT {
            self.web_html = Some(oversize_notice(document.source().len()));
            return;
        }
        if document.doc_type() == DocType::Html {
            // A `file://` document has a real origin, so relative assets and
            // everything else the user can read are reachable. Only explicit
            // trust permits that origin. Restricted content uses the opaque
            // `data:` origin and must not be encoded a second time here.
            // A trusted file URL shows disk state rather than unsaved editor
            // text; live trusted HTML would need a temporary-file protocol.
            self.web_html = Some(match trust {
                Trust::Trusted => source_path
                    .map(web::to_file_url)
                    .unwrap_or_else(|| web::build_html_raw(document, Trust::Restricted)),
                Trust::Restricted => web::build_html_raw(document, trust),
            });
            return;
        }

        // Use the app preset rather than the OS default so explicit themes
        // match the surrounding GPUI chrome.
        self.web_html = Some(web::build_html_themed(
            document,
            &self.registry,
            trust,
            Some(crate::settings::active_preset(cx)),
        ));
    }

    pub(super) fn web_html(&self) -> Option<&str> {
        self.web_html.as_deref()
    }

    pub(super) fn web_payload(&self) -> Option<(&str, u64)> {
        self.web_html
            .as_deref()
            .map(|html| (html, self.web_revision))
    }

    #[cfg(test)]
    pub(super) fn web_revision(&self) -> u64 {
        self.web_revision
    }

    pub(super) fn has_web_failure(&self) -> bool {
        self.web_failure_revision == Some(self.web_revision)
    }

    pub(super) fn mark_web_failed(&mut self, revision: u64) -> bool {
        if self.web_revision == revision && self.web_failure_revision != Some(revision) {
            self.web_failure_revision = Some(revision);
            true
        } else {
            false
        }
    }

    pub(super) fn request_web_retry(&mut self) -> bool {
        if !self.has_web_failure() {
            return false;
        }
        self.web_failure_revision = None;
        true
    }

    pub(super) fn clear_web_failure(&mut self) {
        self.web_failure_revision = None;
    }

    pub(super) fn clear_web_failure_for(&mut self, revision: u64) -> bool {
        if self.web_failure_revision.is_some_and(|failed_revision| {
            failed_revision == revision || revision == self.web_revision
        }) {
            self.web_failure_revision = None;
            true
        } else {
            false
        }
    }
}

/// Markdown extensions that render diagram and math fences through the
/// registry.
/// One parser and renderer handle all registered technologies, so adding a
/// renderer does not require another Markdown language-specific branch.
fn diagram_extensions(registry: Arc<RendererRegistry>) -> MarkdownExtensions {
    let parse_registry = registry.clone();
    MarkdownExtensions::default()
        .block_parser(move |node, _cx| {
            let markdown_ast::Node::Code(code) = node else {
                return None;
            };
            let lang = code.lang.as_deref().unwrap_or("").trim();
            let id = match mt_core::DiagramKind::from_lang(lang) {
                Some(kind) => kind.id().to_string(),
                None if lang.eq_ignore_ascii_case("math")
                    || lang.eq_ignore_ascii_case("latex")
                    || lang.eq_ignore_ascii_case("tex") =>
                {
                    "math".to_string()
                }
                None => return None,
            };
            // This parser runs on the background markdown parse task, so a
            // renderer that shells out never blocks the UI thread.
            let outcome = parse_registry.render(&id, &code.value);
            Some(
                MarkdownNode::new(
                    "mt-block",
                    RenderedBlock {
                        id,
                        outcome,
                        source: code.value.clone(),
                    },
                )
                .markdown(format!("```{lang}\n{}\n```", code.value)),
            )
        })
        .block_renderer("mt-block", move |node, _window, cx| {
            let Some(block) = node.data::<RenderedBlock>() else {
                return div().into_any_element();
            };
            render_block(block, cx)
        })
}

/// A block after the registry has rendered a diagram or math fence.
#[derive(Clone)]
struct RenderedBlock {
    id: String,
    outcome: mt_core::rendering::RenderOutcome,
    source: String,
}

fn render_block(block: &RenderedBlock, cx: &mut App) -> AnyElement {
    use mt_core::rendering::RenderOutcome;

    match &block.outcome {
        // SVG renders natively through resvg.
        RenderOutcome::Svg(markup) if markup.contains("<svg") => div()
            .w_full()
            .flex()
            .justify_center()
            .py_2()
            .child(
                img(Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    themed_svg(markup, cx).into_bytes(),
                )))
                .object_fit(ObjectFit::Contain)
                .max_w_full(),
            )
            .into_any_element(),
        // resvg cannot draw MathML, so preserve the formula source in a math
        // style rather than replacing it with an empty box.
        RenderOutcome::Svg(_) => div()
            .w_full()
            .flex()
            .justify_center()
            .py_2()
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(block.source.trim().to_string()),
            )
            .into_any_element(),
        // A failed renderer reports its diagnostic and leaves the source
        // visible; rendering failure must not lose the user's content.
        RenderOutcome::Failed(diag) => v_flex()
            .w_full()
            .my_2()
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().danger.opacity(0.6))
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(format!("{} rendering failed", block.id))
                    .when_some(diag.line, |this, line| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("line {line}")),
                        )
                    }),
            )
            .child(div().text_xs().child(diag.message.clone()))
            .child(
                div()
                    .mt_1()
                    .p_2()
                    .w_full()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(block.source.clone()),
            )
            .into_any_element(),
    }
}

/// Inject the foreground into SVGs before rasterization so `currentColor`
/// follows the native preview's active theme. The registry cache is keyed by
/// `(id, source)`, not theme, so baking color into its cached SVG would make a
/// later theme switch keep the old color. Native SVG rasterization also falls
/// back to black when `currentColor` has no explicit color to resolve.
fn themed_svg(markup: &str, cx: &App) -> String {
    let fg = cx.theme().foreground.to_rgb();
    let ch = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
    let color = format!("#{:02x}{:02x}{:02x}", ch(fg.r), ch(fg.g), ch(fg.b));
    match markup.find("<svg") {
        Some(_) if markup[..markup.find('>').unwrap_or(markup.len())].contains("color=") => {
            markup.to_string()
        }
        Some(start) => {
            let insert = start + "<svg".len();
            format!(
                "{} color=\"{color}\"{}",
                &markup[..insert],
                &markup[insert..]
            )
        }
        None => markup.to_string(),
    }
}

pub(super) fn native_style(_cx: &App) -> TextViewStyle {
    // Wide tables scroll horizontally rather than wrapping into unreadable
    // narrow columns.
    let mut table = StyleRefinement::default();
    table.overflow.x = Some(Overflow::Scroll);
    TextViewStyle::default().table(table)
}

fn oversize_notice(len: usize) -> String {
    format!(
        "<!doctype html><html><body style=\"font-family:system-ui;padding:2rem\">\
         <h3>Web preview paused</h3>\
         <p>This document is {} MB. Rendering it through the WebView on every edit \
         would block the UI. Native preview and the editor remain fully live.</p>\
         </body></html>",
        len / (1024 * 1024)
    )
}
