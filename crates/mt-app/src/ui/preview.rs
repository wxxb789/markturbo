//! Owner lane: document.
//!
//! The preview and the web compatibility pane, in the column (Preview / Web)
//! and in the field (Split puts the preview in the field, SplitWeb the web
//! pane).
//!
//! Contract: [`column`] and [`field`] draw the rendered document — the shell
//! places whichever the pane asks for; [`is_open`] tells the shell whether the
//! field is showing a preview at all. [`web`] is the compatibility pane.
//!
//! The document lane owns: Martian Grotesk 16/27 for the preview body, the
//! preview's diagram and math rendering (ported here from the old `app.rs`,
//! which is why the helpers below already work), the web pane's trust level,
//! its `web_dirty` deferral (never mutate a WebView from render) and its
//! restricted-scripts banner.

use mt_core::{
    BlockKind, Document,
    rendering::{RenderOutcome, RendererRegistry},
    settings::Language,
};
use nsh_base::{TextViewMath, text_view};
use nsh_core::{element::ImageSrc, prelude::*};

use crate::{
    state::{AppState, Pane, Tab},
    ui::tokens::{self, FONT_UI, TYPE_PREVIEW, TYPE_PREVIEW_LINE},
};

/// Whether the field is showing the preview or the web pane (Split, SplitWeb).
pub fn is_open(state: &AppState) -> bool {
    state
        .active_tab()
        .is_some_and(|tab| matches!(tab.pane.get(), Pane::Split | Pane::SplitWeb))
}

/// The rendered document, in the column.
pub fn column(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    rendered(state)
}

/// The rendered document, in the field, under a quiet label naming the pane.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    view()
        .test_id("preview-field")
        .w_full()
        .flex_col()
        .gap(px(tokens::SPACE_4))
        .child(quiet_label("Preview"))
        .child(rendered(state))
}

/// The web compatibility pane. The document lane ports `web::build_html`, the
/// trust level and the deferred `web_dirty` update into this; until then it
/// says what the pane is and renders no page — it never claims a preview it
/// does not have.
pub fn web(state: AppState, language: Language) -> impl IntoElement {
    let _ = (state, language);
    view()
        .test_id("web-pane")
        .w_full()
        .flex_col()
        .gap(px(tokens::SPACE_3))
        .child(quiet_label("Web (compatibility)"))
        .child(
            text("This pane renders the document in the system's browser engine.")
                .font_family(FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
}

/// A quiet label for a surface that has nothing else to say.
fn quiet_label(label: &'static str) -> impl IntoElement {
    text(label)
        .font_family(FONT_UI)
        .text_size(px(tokens::TYPE_UI))
        .line_height(px(tokens::TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_faint)
}

/// The active document, rendered.
fn rendered(state: AppState) -> impl IntoElement {
    let state_for_tabs = state.clone();
    view().flex_1().h(relative(1.0)).child(each(
        move || state_for_tabs.active_tab().into_iter().collect::<Vec<_>>(),
        |tab: &Tab| tab.id,
        move |tab: ReadSignal<Tab>, _| {
            let tab = tab.get();
            let preview = computed(move || preprocess_markdown(&tab.text.get(), &tab.path));
            scroll().flex_1().child(
                text_view()
                    .markdown(move || Some(SharedString::from(preview.get())))
                    .font_family(FONT_UI)
                    .text_size(px(TYPE_PREVIEW))
                    .line_height(px(TYPE_PREVIEW_LINE))
                    .selectable(true)
                    .render_math(|math: &TextViewMath| render_diagram("math", math.tex.as_ref())),
            )
        },
    ))
}

// ─── ported from the old UI ───────────────────────────────────────────────
//
// These four helpers are view-independent: the preview needs them and nothing
// about Afterglow changes them. The document lane keeps them.

fn renderer() -> &'static RendererRegistry {
    static REGISTRY: std::sync::LazyLock<RendererRegistry> =
        std::sync::LazyLock::new(RendererRegistry::with_defaults);
    &REGISTRY
}

/// Markdown with its diagram blocks replaced by inline SVG images.
fn preprocess_markdown(source: &str, path: &std::path::Path) -> String {
    let document = Document::new(Some(path.to_path_buf()), source.to_owned());
    let mut out = source.to_owned();
    for block in document.blocks().iter().rev() {
        let BlockKind::Diagram(kind) = &block.kind else {
            continue;
        };
        let RenderOutcome::Svg(markup) = renderer().render(kind.id(), &block.content) else {
            continue;
        };
        let replacement = format!(
            "\n<img src=\"{}\" alt=\"{}\">\n",
            svg_data_uri(&markup),
            kind
        );
        out.replace_range(block.range.clone(), &replacement);
    }
    out
}

/// An SVG data URL the preview's image can load.
fn svg_data_uri(markup: &str) -> String {
    use std::fmt::Write;
    let mut uri = String::with_capacity(markup.len() + 32);
    uri.push_str("data:image/svg+xml,");
    for byte in markup.bytes() {
        if byte.is_ascii_alphanumeric() || b" -._~!$'()*+,;=:@/?".contains(&byte) {
            uri.push(byte as char);
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// A diagram or an equation, drawn as its SVG; a failure shows the source in
/// mono rather than nothing.
fn render_diagram(kind: &str, source: &str) -> AnyElement {
    match renderer().render(kind, source) {
        RenderOutcome::Svg(markup) => svg(ImageSrc::svg_markup(&markup)).into_element(),
        RenderOutcome::Failed(_diagnostic) => text(source.to_owned())
            .font_family(tokens::FONT_SOURCE)
            .text_size(px(tokens::TYPE_SOURCE))
            .text_color(move || tokens::palette().ink_faint)
            .into_element(),
    }
}
