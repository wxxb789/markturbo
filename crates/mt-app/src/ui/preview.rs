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
//!
//! The web pane never navigates from render: its [`WebDocument`] watches the
//! document text, the trust level and the appearance, marks the page dirty and
//! builds it from an effect, whose write the web view applies at the next
//! commit. A pane that is not on screen is not there to watch, so a hidden tab
//! is never navigated behind the user's back.

use std::sync::LazyLock;

use mt_core::{
    BlockKind, Document,
    rendering::{RenderOutcome, RendererRegistry},
    settings::Language,
};
use nsh_base::{TextViewMath, text_view};
use nsh_core::{element::ImageSrc, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, Pane, Tab},
    ui::{prims, tokens},
    web,
};

/// The preview body's family: what the app's registered grotesk answers to.
///
/// `tokens::FONT_UI` names it by its typographic family (`Martian Grotesk`),
/// which the Windows text engine does not find — it searches with DirectWrite's
/// `FindFamilyName`, which matches the file's *legacy* family name (nameID 1:
/// `Martian Grotesk Std Rg`, the Regular instance the 16/27 body wants).
/// Measured in the editor's rows, 60 `M`s at 14pt: the registered face 884,
/// the fallback 755, DengXian 727 — see
/// `document::tests::the_editor_draws_the_registered_source_font`. The
/// fallbacks follow the spec's list.
const FONT_PREVIEW: &str = "Martian Grotesk Std Rg, DengXian, Microsoft YaHei UI, sans-serif";

/// Whether the field is showing the preview or the web pane (Split, SplitWeb).
pub fn is_open(state: &AppState) -> bool {
    state
        .active_tab()
        .is_some_and(|tab| matches!(tab.pane.get(), Pane::Split | Pane::SplitWeb))
}

/// The rendered document, in the column.
pub fn column(state: AppState, language: Language) -> impl IntoElement {
    view()
        .test_id("preview-column")
        .flex_1()
        .min_h(px(0.0))
        .h(relative(1.0))
        .child(rendered(state, language))
}

/// The rendered document, in the field, under a quiet label naming the pane.
pub fn field(state: AppState, language: Language) -> AnyElement {
    let pane_state = state.clone();
    let web_state = state.clone();
    let preview_state = state.clone();
    view()
        .test_id("preview-field")
        .flex_1()
        .min_h(px(0.0))
        .h(relative(1.0))
        .flex_col()
        .gap(px(tokens::SPACE_3))
        // The field's docked column starts 40px in, as the prototype's
        // `.field > *` does.
        .pl(px(tokens::SPACE_5))
        .child(quiet_label(move || {
            if pane_state
                .active_tab()
                .is_some_and(|tab| matches!(tab.pane.get(), Pane::SplitWeb))
            {
                SharedString::from(i18n::t(language, Key::ModeWeb))
            } else {
                SharedString::from(i18n::t(language, Key::ModeNative))
            }
        }))
        .child(
            when(
                {
                    let condition_state = web_state.clone();
                    move || {
                        condition_state
                            .active_tab()
                            .is_some_and(|tab| matches!(tab.pane.get(), Pane::SplitWeb))
                    }
                },
                move || web(web_state.clone(), language).into_element(),
            )
            .otherwise(move || rendered(preview_state.clone(), language).into_element()),
        )
        .into_element()
}

/// The web compatibility pane: the document as a page, with its trust level,
/// in the column (Web) or in the field (SplitWeb).
///
/// The pane renders nothing it does not have: while Restricted the page's
/// scripts do not run and the banner says so; `Trust scripts` hands the file
/// to the state, which asks the user before anything changes.
pub fn web(state: AppState, language: Language) -> AnyElement {
    let Some(tab) = state.active_tab() else {
        return quiet_label(move || SharedString::from(i18n::t(language, Key::ModeWeb)))
            .into_element();
    };
    let page = WebDocument::new(tab.clone());
    let trusted = tab.web_trusted;
    let banner_state = state.clone();
    let banner_tab = tab.clone();
    view()
        .test_id("web-pane")
        .flex_1()
        .min_h(px(0.0))
        .h(relative(1.0))
        .flex_col()
        .child(
            view()
                .test_id("web-banner")
                .flex_row()
                .items_center()
                .justify_between()
                .w_full()
                .pb(px(tokens::SPACE_2))
                .child(prims::rest(move || {
                    if trusted.get() {
                        SharedString::from(i18n::t(language, Key::ScriptsTrusted))
                    } else {
                        SharedString::from(i18n::t(language, Key::ScriptsRestricted))
                    }
                }))
                .child(when(move || !trusted.get(), {
                    let id = banner_tab.id;
                    move || {
                        let state = banner_state.clone();
                        prims::quiet_action(
                            "web-trust",
                            i18n::t(language, Key::TrustScripts),
                            move || state.request_web_trust(id),
                        )
                        .into_element()
                    }
                })),
        )
        .child(
            web_view_html(move || page.html.get(), None)
                .test_id("web-view")
                .label(SharedString::from(i18n::t(language, Key::ModeWeb)))
                .java_script_enabled(move || trusted.get())
                .flex_1(),
        )
        .into_element()
}

/// A quiet label for a surface that has nothing else to say.
fn quiet_label(label: impl IntoReactive<SharedString>) -> impl IntoElement {
    view().pt(px(tokens::SPACE_2)).child(prims::rest(label))
}

/// The active document, rendered: Markdown with its diagrams inlined, in
/// Martian Grotesk 16/27, selectable, scrolling on its own.
fn rendered(state: AppState, language: Language) -> impl IntoElement {
    // Which document is on screen is read here, not where the surface is
    // built: a tab opens on a task, and the preview has to appear with it.
    let state_for_tabs = state.clone();
    each(
        move || state_for_tabs.active_tab().into_iter().collect::<Vec<_>>(),
        |tab: &Tab| tab.id,
        move |tab: ReadSignal<Tab>, _| {
            let tab = tab.get();
            let preview = preview_of(&tab);
            view()
                .test_id("preview")
                .flex_1()
                .min_h(px(0.0))
                .h(relative(1.0))
                .child(
                    scroll()
                        .test_id("preview-text")
                        .label(SharedString::from(i18n::t(language, Key::ModeNative)))
                        .flex_1()
                        .child(
                            text_view()
                                .markdown(move || Some(SharedString::from(preview.get())))
                                .selectable(true)
                                .render_math(|math: &TextViewMath| {
                                    render_diagram("math", math.tex.as_ref())
                                })
                                .font_family(FONT_PREVIEW)
                                .text_size(px(tokens::TYPE_PREVIEW))
                                .line_height(px(tokens::TYPE_PREVIEW_LINE))
                                .text_color(move || tokens::palette().ink)
                                .pb(px(tokens::SPACE_7)),
                        ),
                )
                .into_element()
        },
    )
}

/// The Markdown the preview shows: the tab's text with every diagram block
/// replaced by its SVG, recomputed when the text changes.
fn preview_of(tab: &Tab) -> Memo<String> {
    let tab = tab.clone();
    computed(move || preprocess_markdown(&tab.text.get(), &tab.path))
}

// ─── the web pane's deferred document ─────────────────────────────────────

/// The page a web pane shows: [`Self::html`] is what the web view binds, and
/// it is written only from an effect — never during render — once the
/// [`Page`] it was built from differs from the one on screen.
struct WebDocument {
    /// What the web view binds; a new value reaches it at the next commit.
    html: Signal<String>,
}

/// What a built page depends on: the document text, the trust level and the
/// appearance.
#[derive(Clone, PartialEq)]
struct Page {
    text: String,
    trusted: bool,
    dark: bool,
}

impl WebDocument {
    /// Watches the tab's text, trust level and the appearance, and rebuilds the
    /// page from an effect when any of them changes: the write to
    /// [`Self::html`] is what marks it dirty, and the web view applies it after
    /// the commit that sees the change, so a WebView is never navigated from
    /// render.
    fn new(tab: Tab) -> WebDocument {
        let html = signal(String::new());
        let written = signal(None::<Page>);
        let watch = tab.clone();
        let build = tab.clone();
        effect(move || {
            let page = Page {
                text: watch.text.get().to_string(),
                trusted: watch.web_trusted.get(),
                dark: dark(),
            };
            if written.get_untracked().as_ref() == Some(&page) {
                return;
            }
            let document = Document::new(Some(build.path.clone()), page.text.clone());
            let trust = if page.trusted {
                web::Trust::Trusted
            } else {
                web::Trust::Restricted
            };
            let built = web::build_html(&document, renderer(), trust, page.dark);
            written.set(Some(page));
            html.set(built);
        });
        WebDocument { html }
    }
}

// ─── ported from the old UI ───────────────────────────────────────────────
//
// These four helpers are view-independent: the preview needs them and nothing
// about Afterglow changes them. The document lane keeps them.

fn renderer() -> &'static RendererRegistry {
    static REGISTRY: LazyLock<RendererRegistry> = LazyLock::new(RendererRegistry::with_defaults);
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
        RenderOutcome::Failed(_diagnostic) => {
            prims::source_ink(source.to_owned(), move || tokens::palette().ink_rest).into_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;
    use crate::state::AppState;

    /// The fixture the probes open: a heading, inline code and body lines.
    fn probe_path() -> &'static std::path::Path {
        static FIXTURE: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("probe.md");
                let mut text = String::from("# Release plan\n\nthe `agent` runs\n");
                for index in 0..20 {
                    text.push_str(&format!("line {index}\n"));
                }
                std::fs::write(&path, text).expect("write the fixture");
                (dir, path)
            })
            .1
            .as_path()
    }

    /// The state with the fixture open (the tab arrives on a task).
    fn probe_state() -> AppState {
        let state = AppState::new();
        if let Some(parent) = probe_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(probe_path().to_path_buf());
        state
    }

    /// The rendered document in the column, in a column-sized box.
    #[component]
    fn ColumnProbe() -> impl IntoElement {
        let state = probe_state();
        view()
            .test_id("probe-column")
            .w(px(680.0))
            .h(px(700.0))
            .child(column(state, Language::English))
    }

    /// The rendered document in the field, in a field-sized box.
    #[component]
    fn FieldProbe() -> impl IntoElement {
        let state = probe_state();
        view()
            .test_id("probe-field")
            .w(px(352.0))
            .h(px(700.0))
            .child(field(state, Language::English))
    }

    /// The rendered document alone, without the field's label: the smallest
    /// case the field builds on.
    #[component]
    fn ContentProbe() -> impl IntoElement {
        let state = probe_state();
        view()
            .test_id("probe-content")
            .w(px(352.0))
            .h(px(700.0))
            .child(rendered(state, Language::English))
    }

    /// What a probe put on screen, for a failure message.
    fn dump(app: &Headless) -> String {
        app.screen()
            .layout_nodes()
            .iter()
            .filter(|node| node.test_id.is_some() || node.text.is_some())
            .map(|node| {
                format!(
                    "{:?} {}x{} @y{} text={:?}",
                    node.test_id.as_deref(),
                    node.bounds.size.width.0,
                    node.bounds.size.height.0,
                    node.bounds.origin.y.0,
                    node.text.as_ref().map(|t| t.text.text.as_str())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn texts(app: &Headless) -> String {
        app.screen().texts().join("\n")
    }

    /// A diagram block leaves the preview as an inline SVG image the text view
    /// can load, not as its fence; the rest of the document is untouched.
    #[test]
    fn a_diagram_becomes_an_inline_image() {
        let source = "# Plan\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nAfter.\n";
        let rendered = preprocess_markdown(source, std::path::Path::new("plan.md"));
        assert!(rendered.contains("data:image/svg+xml,"), "{rendered}");
        assert!(!rendered.contains("```mermaid"), "{rendered}");
        assert!(rendered.starts_with("# Plan\n\n"), "{rendered}");
        assert!(rendered.ends_with("\nAfter.\n"), "{rendered}");
    }

    /// A block no renderer handles (an unknown fence language) stays as it
    /// was written.
    #[test]
    fn an_unknown_fence_stays_verbatim() {
        let source = "```nonexistent\nkeep me\n```\n";
        assert_eq!(
            preprocess_markdown(source, std::path::Path::new("plan.md")),
            source
        );
    }

    /// The preview renders the document's own content — a heading, its inline
    /// code and its body — in the column, and keeps the source out of it.
    #[test]
    fn the_preview_renders_the_document() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ColumnProbe>())).expect("start the probe");
        app.wait_for("id:preview").expect("the preview renders");
        let shown = texts(&app);
        assert!(shown.contains("Release plan"), "{}", dump(&app));
        assert!(shown.contains("agent"), "{}", dump(&app));
        assert!(!shown.contains("# Release plan"), "{}", dump(&app));

        // The preview body is Martian Grotesk 16/27 (the registered Regular
        // instance, under the name the text engine finds it by).
        let body = app
            .screen()
            .span_style("text:line 0")
            .expect("a body line's style");
        assert_eq!(body.font_size.0, tokens::TYPE_PREVIEW);
        assert_eq!(body.line_height.0, tokens::TYPE_PREVIEW_LINE);
        let family = body
            .font_family
            .as_ref()
            .map(|family| family.name().to_string())
            .unwrap_or_default();
        assert!(
            family.contains("Martian Grotesk"),
            "the preview body is set in {family:?}"
        );
    }

    /// The same document renders in the field's docked column (Split), under
    /// the pane's quiet label.
    #[test]
    fn the_field_shows_the_preview() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<FieldProbe>())).expect("start the probe");
        app.wait_for("id:preview-field").expect("the field renders");
        // The fixture opens on a task: wait for the document itself.
        app.wait_for("id:preview").expect("the preview renders");
        let shown = texts(&app);
        assert!(shown.contains("Preview"), "{}", dump(&app));
        assert!(shown.contains("Release plan"), "{}", dump(&app));
    }

    /// The field's content alone renders too: whatever the field adds around
    /// it, the document is there.
    #[test]
    fn the_field_content_renders_alone() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ContentProbe>()))
            .expect("start the probe");
        app.wait_for("id:preview").expect("the preview renders");
        assert!(texts(&app).contains("Release plan"), "{}", dump(&app));
    }

    /// The whole app, with a document opened in Split: the shell's field holds
    /// the preview, not just its label.
    #[test]
    fn the_shell_shows_the_preview_in_the_field() {
        // SAFETY: this binary's tests are the only readers of the variable, and
        // this is the only test that sets it (the probes build their own root).
        unsafe { std::env::set_var("MARKTURBO_OPEN", probe_path()) };
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<crate::App>())).expect("start the app");
        app.wait_for("label:Markdown source")
            .expect("the source editor");
        app.wait_for("id:preview-field").expect("the field renders");
        app.wait_for("text:Release plan")
            .expect("the document renders");
        let shown = texts(&app);
        assert!(shown.contains("Preview"), "{}", dump(&app));
        assert!(shown.contains("Release plan"), "{}", dump(&app));
        println!("field nodes:\n{}", dump(&app));
    }
}
