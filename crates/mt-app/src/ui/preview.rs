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
//! The preview body uses [`tokens::FONT_UI`], which leads with the Regular
//! instance's legacy family so DirectWrite finds the registered face.
//! The Windows editor probe (`document::tests::the_editor_draws_the_registered_source_font`)
//! measured 60 `M`s at 14 px in editor rows, not in the preview's TextView:
//! 884 for both `FONT_UI` and the registered Regular face, 755 for a missing
//! family, and 727 for DengXian.
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
use nsh_base::{TextViewMath, TextViewPart, TextViewParts, text_view};
use nsh_core::{
    element::{ImageSrc, StyleRefinement},
    prelude::*,
};

use crate::{
    i18n::{self, Key},
    state::{AppState, Pane, Tab},
    ui::{
        prims,
        tokens::{self, WEIGHT_MEDIUM},
    },
    web,
};

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
                // Keyed by the active tab, as the column and the native
                // preview are: switching between two SplitWeb tabs remounts
                // the pane on the new document, its page and its trust.
                move || {
                    let tabs_state = web_state.clone();
                    let pane_state = web_state.clone();
                    each(
                        move || tabs_state.active_tab().into_iter().collect::<Vec<_>>(),
                        |tab: &Tab| tab.id,
                        move |_, _| web(pane_state.clone(), language),
                    )
                    .into_element()
                },
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
    let trusted = tab.web_trusted;
    let text = tab.text;
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
            when(
                move || text.get().trim().is_empty(),
                move || empty_preview(language),
            )
            .otherwise(move || {
                web_document(WebDocument::new(tab.clone(), language), trusted, language)
            }),
        )
        .into_element()
}

/// A quiet label for a surface that has nothing else to say.
fn quiet_label(label: impl IntoReactive<SharedString>) -> impl IntoElement {
    view().pt(px(tokens::SPACE_2)).child(prims::rest(label))
}

fn empty_preview(language: Language) -> AnyElement {
    view()
        .test_id("preview-empty")
        .flex_1()
        .min_h(px(0.0))
        .p(px(tokens::SPACE_4))
        .child(prims::faint(SharedString::from(i18n::t(
            language,
            Key::PreviewEmpty,
        ))))
        .into_element()
}

fn web_load_message(test_id: &str, message: impl IntoElement) -> AnyElement {
    view()
        .test_id(test_id)
        .px(px(tokens::SPACE_4))
        .py(px(tokens::SPACE_2))
        .child(message)
        .into_element()
}

fn web_document(page: WebDocument, trusted: Signal<bool>, language: Language) -> AnyElement {
    let WebDocument { html, status } = page;
    view()
        .test_id("web-content")
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(dynamic(move || match status.get() {
            WebLoadState::Loading => web_load_message(
                "preview-loading",
                prims::faint(i18n::t(language, Key::PreviewLoading)),
            ),
            WebLoadState::Failed => web_load_message(
                "web-preview-failed",
                prims::rest(i18n::t(language, Key::WebPreviewFailed))
                    .text_color(move || tokens::palette().danger),
            ),
            WebLoadState::Ready => view().into_element(),
        }))
        .child(
            web_view_html(move || html.get(), None)
                .test_id("web-view")
                .label(SharedString::from(i18n::t(language, Key::ModeWeb)))
                .java_script_enabled(move || trusted.get())
                .on_load_start(move |_: &WebNavigation| web_load_started(status))
                .on_load(move |_: &WebNavigation| web_load_finished(status))
                .on_error(move |_: &str| web_load_failed(status))
                .flex_1(),
        )
        .into_element()
}

fn web_load_started(status: Signal<WebLoadState>) {
    if status.get() != WebLoadState::Ready {
        status.set(WebLoadState::Loading);
    }
}

fn web_load_finished(status: Signal<WebLoadState>) {
    status.set(WebLoadState::Ready);
}

fn web_load_failed(status: Signal<WebLoadState>) {
    status.set(WebLoadState::Failed);
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
            let preview = preview_of(&tab, language);
            let text = tab.text;
            provide_context(preview_parts());
            view()
                .test_id("preview")
                .flex_1()
                .min_h(px(0.0))
                .h(relative(1.0))
                .child(
                    when(
                        move || text.get().trim().is_empty(),
                        move || empty_preview(language),
                    )
                    .otherwise(move || {
                        scroll()
                            .test_id("preview-text")
                            .label(SharedString::from(i18n::t(language, Key::ModeNative)))
                            .flex_1()
                            .child(
                                text_view()
                                    .markdown(move || Some(SharedString::from(preview.get())))
                                    .selectable(true)
                                    .render_math(move |math: &TextViewMath| {
                                        render_diagram("math", "Math", math.tex.as_ref(), language)
                                    })
                                    .font_family(tokens::FONT_UI)
                                    .text_size(px(tokens::TYPE_PREVIEW))
                                    .line_height(px(tokens::TYPE_PREVIEW_LINE))
                                    .text_color(move || tokens::palette().ink)
                                    .pb(px(tokens::SPACE_7)),
                            )
                            .into_element()
                    }),
                )
                .into_element()
        },
    )
}

/// The preview's calm typography: the model's 22px light stays the only large
/// size in the app, so headings step up from the 16/27 body by size and the
/// Medium instance, never by a synthetic bold.
///
/// * H1 is 20/28, H2 18/27, the rest at the body's 16/27 — all Martian
///   Grotesk Medium (500), with no tracking of their own;
/// * strong text is Medium (500), not text_view's semibold;
/// * inline code is at the source's size: text_view's own is 13pt, a step
///   below the source column. Its face is the theme's `font-mono`
///   (theme.json), the source's Martian Mono;
/// * a task's box is a 12px square, radius 0 like every corner here, centred
///   on the body's first line.
fn preview_parts() -> TextViewParts {
    let medium = |style: StyleRefinement| {
        style
            .font_family(tokens::FONT_MEDIUM)
            .font_weight(WEIGHT_MEDIUM)
    };
    // Every heading is Medium at the body's size; the level's own part
    // comes after it and lifts H1 and H2.
    TextViewParts::new()
        .part(TextViewPart::Heading, move |style| {
            medium(style)
                .text_size(px(tokens::TYPE_PREVIEW))
                .line_height(px(tokens::TYPE_PREVIEW_LINE))
                .letter_spacing(px(0.0))
        })
        .part(TextViewPart::Heading1, |style| {
            style
                .text_size(px(PREVIEW_H1))
                .line_height(px(PREVIEW_H1_LINE))
        })
        .part(TextViewPart::Heading2, |style| {
            style.text_size(px(PREVIEW_H2))
        })
        .part(TextViewPart::Strong, medium)
        .part(TextViewPart::Code, |style| {
            style.text_size(px(tokens::TYPE_SOURCE))
        })
        .part(TextViewPart::TaskMarker, |style| {
            style
                .size(px(TASK_BOX))
                .mt(px((tokens::TYPE_PREVIEW_LINE - TASK_BOX) / 2.0))
                .rounded(px(0.0))
                .border(px(tokens::HAIRLINE))
        })
}

/// The preview's first heading: 20/28, two steps under the model's 22/30.
const PREVIEW_H1: f32 = 20.0;
const PREVIEW_H1_LINE: f32 = 28.0;
/// The second heading, one step above the body.
const PREVIEW_H2: f32 = 18.0;
/// A task's box: a 12px square.
const TASK_BOX: f32 = 12.0;

fn preview_of(tab: &Tab, language: Language) -> Memo<String> {
    let tab = tab.clone();
    computed(move || preprocess_markdown(&tab.text.get(), &tab.path, language))
}

// ─── the web pane's deferred document ─────────────────────────────────────

/// The page a web pane shows: [`Self::html`] is what the web view binds, and
/// it is written only from an effect — never during render — once the
/// [`Page`] it was built from differs from the one on screen.
#[derive(Clone, Copy)]
struct WebDocument {
    /// What the web view binds; a new value reaches it at the next commit.
    html: Signal<String>,
    status: Signal<WebLoadState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WebLoadState {
    Loading,
    Ready,
    Failed,
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
    fn new(tab: Tab, language: Language) -> WebDocument {
        let html = signal(String::new());
        let status = signal(WebLoadState::Loading);
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
            let built =
                web::build_html_localized(&document, renderer(), trust, page.dark, language);
            written.set(Some(page));
            html.set(built);
        });
        WebDocument { html, status }
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

fn preprocess_markdown(source: &str, path: &std::path::Path, language: Language) -> String {
    let document = Document::new(Some(path.to_path_buf()), source.to_owned());
    let mut out = source.to_owned();
    for block in document.blocks().iter().rev() {
        let BlockKind::Diagram(kind) = &block.kind else {
            continue;
        };
        let replacement = match renderer().render(kind.id(), &block.content) {
            RenderOutcome::Svg(markup) => format!(
                "\n<img src=\"{}\" alt=\"{}\">\n",
                svg_data_uri(&markup),
                kind
            ),
            RenderOutcome::Failed(_) => format!(
                "\n> **{}**\n\n{}",
                i18n::diagram_failure_label(language, &kind.to_string()),
                &source[block.range.clone()]
            ),
        };
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

fn render_diagram(kind: &str, label: &str, source: &str, language: Language) -> AnyElement {
    match renderer().render(kind, source) {
        RenderOutcome::Svg(markup) => svg(ImageSrc::svg_markup(&markup)).into_element(),
        RenderOutcome::Failed(_) => view()
            .test_id("diagram-failure")
            .flex_col()
            .gap(px(tokens::SPACE_2))
            .child(prims::faint(i18n::diagram_failure_label(language, label)))
            .child(prims::source_ink(source.to_owned(), move || {
                tokens::palette().ink_rest
            }))
            .into_element(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nsh_core::{EventKind, EventPayload, MountItem, NativeProps, WebSource};
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;
    use crate::state::AppState;

    const INVALID_DIAGRAM: &str = "```mermaid\nnot a diagram\n```\n";

    /// The fixture the probes open: headings, inline code, strong text, two
    /// tasks and body lines.
    fn probe_path() -> &'static std::path::Path {
        static FIXTURE: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("probe.md");
                let mut text = String::from(
                    "# Release plan\n\nthe `agent` runs\n\nlist **one** risk\n\n## Scope\n\n\
                     - [ ] verify install\n- [x] verify uninstall\n\n",
                );
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

    fn failure_path() -> &'static std::path::Path {
        static FIXTURE: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("failure.md");
                std::fs::write(&path, INVALID_DIAGRAM).expect("write the invalid diagram");
                (dir, path)
            })
            .1
            .as_path()
    }

    fn failure_state() -> AppState {
        let state = AppState::new();
        if let Some(parent) = failure_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(failure_path().to_path_buf());
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

    #[component]
    fn EmptyProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from(" \n\t"));
        }
        view()
            .test_id("probe-empty")
            .w(px(352.0))
            .h(px(700.0))
            .child(rendered(state, Language::English))
    }

    #[component]
    fn EmptyWebProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        view()
            .test_id("probe-empty-web")
            .w(px(680.0))
            .h(px(700.0))
            .child(web(state, Language::English))
    }

    #[component]
    fn PendingWebProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let text = state.active_tab().expect("document").text;
        text.set("First document".into());
        view()
            .test_id("probe-pending-web")
            .w(px(352.0))
            .h(px(700.0))
            .flex_col()
            .child(prims::quiet_action(
                "probe-web-clear",
                "Clear document",
                move || text.set("".into()),
            ))
            .child(prims::quiet_action(
                "probe-web-type",
                "Type document",
                move || text.set("Second document".into()),
            ))
            .child(web(state, Language::English))
    }

    #[component]
    fn DiagramFailureProbe() -> impl IntoElement {
        view()
            .test_id("probe-diagram-failure")
            .w(px(680.0))
            .h(px(700.0))
            .flex_col()
            .child(rendered(failure_state(), Language::English))
            .child(rendered(failure_state(), Language::Chinese))
    }

    #[component]
    fn WebDiagramFailureProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let source = state.active_tab().expect("diagram document").text;
        view()
            .flex_col()
            .child(prims::quiet_action(
                "type-diagram",
                "Type diagram",
                move || {
                    source.set(INVALID_DIAGRAM.into());
                },
            ))
            .child(web(state, Language::Chinese))
    }

    /// Two documents in SplitWeb: the first trusted, the second (active)
    /// restricted, and a button that switches to the first.
    #[component]
    fn SplitWebProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        state.new_document();
        let tabs = state.tabs.get();
        for tab in &tabs {
            tab.pane.set(Pane::SplitWeb);
        }
        tabs[0].text.set(SharedString::from("Trusted page"));
        tabs[1].text.set(SharedString::from("Restricted page"));
        tabs[0].web_trusted.set(true);
        let first = tabs[0].id;
        let switch = state.clone();
        view()
            .test_id("probe-split-web")
            .w(px(352.0))
            .h(px(700.0))
            .flex_col()
            .child(prims::quiet_action(
                "probe-switch",
                "Probe switch",
                move || switch.set_active(first),
            ))
            .child(field(state, Language::English))
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

    fn matches(app: &Headless, selector: &str) -> bool {
        app.screen().matches(selector).expect("selector")
    }

    fn dispatch_web_event(app: &mut Headless, event: EventKind, payload: EventPayload) {
        let node = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("web-view"))
            .expect("mounted WebView");
        app.engine()
            .control()
            .dispatch_event(node.id, event, payload);
        app.settle();
    }

    /// A diagram block leaves the preview as an inline SVG image the text view
    /// can load, not as its fence; the rest of the document is untouched.
    #[test]
    fn a_diagram_becomes_an_inline_image() {
        let source = "# Plan\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nAfter.\n";
        let rendered =
            preprocess_markdown(source, std::path::Path::new("plan.md"), Language::English);
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
            preprocess_markdown(source, std::path::Path::new("plan.md"), Language::English),
            source
        );
    }

    #[test]
    fn an_empty_preview_has_a_designed_state() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<EmptyProbe>())).expect("start the probe");
        app.wait_for("id:preview").expect("the preview exists");
        app.settle();
        assert!(
            matches(&app, "id:preview-empty")
                && texts(&app).contains(i18n::t(Language::English, Key::PreviewEmpty))
                && !matches(&app, "id:preview-text"),
            "{}",
            dump(&app)
        );
    }

    #[test]
    fn an_empty_web_preview_has_a_designed_state() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<EmptyWebProbe>()))
            .expect("start the probe");
        app.wait_for("id:web-pane").expect("the web pane renders");
        app.settle();
        assert!(
            matches(&app, "id:preview-empty")
                && texts(&app).contains(i18n::t(Language::English, Key::PreviewEmpty))
                && !matches(&app, "id:web-view"),
            "{}",
            dump(&app)
        );
    }

    #[test]
    fn a_web_preview_tracks_loading_success_and_failure() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<PendingWebProbe>()))
            .expect("start the probe");
        app.settle();
        assert!(
            matches(&app, "id:preview-loading")
                && texts(&app).contains(i18n::t(Language::English, Key::PreviewLoading))
                && matches(&app, "id:web-view"),
            "{}",
            dump(&app)
        );
        dispatch_web_event(&mut app, EventKind::Load, EventPayload::None);
        assert!(
            matches(&app, "id:web-view") && !matches(&app, "id:preview-loading"),
            "{}",
            dump(&app)
        );
        dispatch_web_event(&mut app, EventKind::LoadStart, EventPayload::None);
        assert!(
            matches(&app, "id:web-view")
                && !matches(&app, "id:preview-loading")
                && !matches(&app, "id:web-preview-failed"),
            "later edits do not repeat the initial loading state: {}",
            dump(&app)
        );
        app.press("id:probe-web-clear").expect("clear the document");
        app.wait_for("id:preview-empty").expect("empty preview");
        assert!(!matches(&app, "id:web-view"), "{}", dump(&app));
        app.press("id:probe-web-type").expect("retype the document");
        app.wait_for("id:web-view").expect("remounted WebView");
        assert!(matches(&app, "id:preview-loading"), "{}", dump(&app));
        dispatch_web_event(
            &mut app,
            EventKind::Error,
            EventPayload::Error {
                message: "load failed".into(),
            },
        );
        assert!(
            matches(&app, "id:web-view")
                && matches(&app, "id:web-preview-failed")
                && !matches(&app, "id:preview-loading"),
            "{}",
            dump(&app)
        );
        dispatch_web_event(&mut app, EventKind::LoadStart, EventPayload::None);
        assert!(matches(&app, "id:preview-loading"), "{}", dump(&app));
        dispatch_web_event(&mut app, EventKind::Load, EventPayload::None);
        assert!(!matches(&app, "id:preview-loading"), "{}", dump(&app));
    }

    #[test]
    fn a_failed_diagram_is_labelled_and_keeps_its_source() {
        let mut app = Headless::start(EngineConfig::new(
            Root::of::<DiagramFailureProbe>().tokens(tokens::theme()),
        ))
        .expect("start the probe");
        app.wait_for("id:preview").expect("the preview exists");
        app.settle();
        let shown = texts(&app);
        assert!(
            shown.contains("Mermaid could not render")
                && shown.contains("Mermaid 无法渲染")
                && shown.contains("not a diagram"),
            "{}",
            dump(&app)
        );
    }

    #[test]
    fn a_failed_web_diagram_is_labelled_and_keeps_its_source() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<WebDiagramFailureProbe>()))
            .expect("start the probe");
        app.wait_for("id:preview-empty").expect("empty web preview");
        let (sent, received) = std::sync::mpsc::channel();
        app.observe(move |_, batch| {
            for item in &batch.items {
                if let MountItem::NativeProps {
                    props: NativeProps::WebView(props),
                    ..
                } = item
                    && let WebSource::Html { html, .. } = &props.source
                    && !html.is_empty()
                {
                    sent.send(html.clone()).expect("capture WebView HTML");
                }
            }
        });
        app.press("id:type-diagram").expect("type the diagram");
        app.settle();
        let html = received.try_iter().last().expect("WebView receives HTML");
        assert!(
            html.contains(r#"class="mt-error-title">Mermaid 无法渲染.</div>"#),
            "{html}"
        );
        assert!(html.contains("<pre>not a diagram"), "{html}");
    }

    /// The preview renders the document's own content — a heading, its inline
    /// code and its body — in the column, and keeps the source out of it.
    #[test]
    fn the_preview_renders_the_document() {
        let mut app = Headless::start(EngineConfig::new(
            Root::of::<ColumnProbe>().tokens(tokens::theme()),
        ))
        .expect("start the probe");
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

        // Inline code reads at the source's own size and face, not
        // text_view's 13pt system monospace.
        let code = app
            .screen()
            .span_style("text:agent")
            .expect("the inline code's style");
        assert_eq!(code.font_size.0, tokens::TYPE_SOURCE);
        let family = code
            .font_family
            .as_ref()
            .map(|family| family.name().to_string())
            .unwrap_or_default();
        assert!(
            family.contains("Martian Mono"),
            "inline code is set in {family:?}"
        );
    }

    /// The preview's typography stays under the model's 22px light: H1 is
    /// 20/28 and H2 18, both in the Medium instance; strong text is Medium
    /// (500), not a synthetic bold; a task's box is a 12px square.
    #[test]
    fn the_preview_typography_is_calm() {
        let mut app = Headless::start(EngineConfig::new(
            Root::of::<ColumnProbe>().tokens(tokens::theme()),
        ))
        .expect("start the probe");
        app.wait_for("id:preview").expect("the preview renders");
        app.settle();
        let medium = |style: &nsh_core::ResolvedTextStyle| {
            style
                .font_family
                .as_ref()
                .map(|family| family.name().to_string())
                .unwrap_or_default()
                .starts_with("Martian Grotesk Std Md")
        };

        let h1 = app
            .screen()
            .span_style("text:Release plan")
            .expect("the first heading's style");
        assert_eq!(
            (h1.font_size.0, h1.line_height.0, h1.font_weight),
            (PREVIEW_H1, PREVIEW_H1_LINE, 500),
            "H1"
        );
        assert!(
            h1.font_size.0 < tokens::TYPE_MODEL,
            "H1 under the model's size"
        );
        assert!(medium(&h1), "H1 in {:?}", h1.font_family);

        let h2 = app
            .screen()
            .span_style("text:Scope")
            .expect("the second heading's style");
        assert_eq!((h2.font_size.0, h2.font_weight), (PREVIEW_H2, 500), "H2");
        assert!(medium(&h2), "H2 in {:?}", h2.font_family);

        let strong = app
            .screen()
            .span_style("text:one")
            .expect("the strong text's style");
        assert_eq!(strong.font_weight, 500, "strong text's weight");
        assert!(medium(&strong), "strong text in {:?}", strong.font_family);
        assert_eq!(strong.font_size.0, tokens::TYPE_PREVIEW);

        // The two task boxes are 12px squares; text_view's 15px box is gone.
        let boxes = |side: f32| {
            app.screen()
                .layout_nodes()
                .iter()
                .filter(|node| {
                    node.text.is_none()
                        && (node.bounds.size.width.0 - side).abs() < 0.01
                        && (node.bounds.size.height.0 - side).abs() < 0.01
                })
                .count()
        };
        assert_eq!(boxes(TASK_BOX), 2, "{}", dump(&app));
        assert_eq!(boxes(15.0), 0, "{}", dump(&app));
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

    /// Switching straight between two SplitWeb tabs rebinds the field's web
    /// pane to the new document: its trust banner follows the active tab.
    #[test]
    fn the_field_web_pane_follows_the_active_tab() {
        let restricted = i18n::t(Language::English, Key::ScriptsRestricted);
        let trusted = i18n::t(Language::English, Key::ScriptsTrusted);
        let mut app = Headless::start(EngineConfig::new(Root::of::<SplitWebProbe>()))
            .expect("start the probe");
        app.wait_for("id:web-pane").expect("the web pane renders");
        app.settle();
        assert!(
            matches(&app, "id:web-view") && matches(&app, "id:preview-loading"),
            "the first page stays mounted while it loads: {}",
            dump(&app)
        );
        let shown = texts(&app);
        assert!(shown.contains(restricted), "{}", dump(&app));
        assert!(!shown.contains(trusted), "{}", dump(&app));

        app.press("label:Probe switch")
            .expect("switch to the first tab");
        app.settle();
        assert!(
            matches(&app, "id:web-view") && matches(&app, "id:preview-loading"),
            "the next tab's page stays mounted while it loads: {}",
            dump(&app)
        );
        let shown = texts(&app);
        assert!(shown.contains(trusted), "{}", dump(&app));
        assert!(!shown.contains(restricted), "{}", dump(&app));
        assert!(
            app.screen()
                .layout_nodes()
                .iter()
                .all(|node| node.test_id.as_deref() != Some("web-trust")),
            "a trusted document offers no Trust scripts action: {}",
            dump(&app)
        );
    }

    /// The shell with the fixture open in Split — chosen here, not left to
    /// the default pane — re-keyed at each breakpoint as `app.rs` re-keys it.
    #[component]
    fn SplitShellProbe() -> impl IntoElement {
        use crate::ui::{shell, tokens::Layout};
        let state = probe_state();
        let split_state = state.clone();
        effect(move || {
            if let Some(tab) = split_state.active_tab() {
                tab.pane.set(Pane::Split);
            }
        });
        each(
            move || vec![Layout::of(shell::client_width_now())],
            |layout: &Layout| format!("{layout:?}"),
            move |layout: ReadSignal<Layout>, _| {
                shell::shell(state.clone(), Language::English, layout.get()).into_element()
            },
        )
    }

    /// The shell with a document open in Split: the field holds the preview,
    /// not just its label.
    #[test]
    fn the_shell_shows_the_preview_in_the_field() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<SplitShellProbe>()))
            .expect("start the probe");
        app.set_viewport(nsh_core::Size::new(
            nsh_core::Points(1000.0),
            nsh_core::Points(900.0),
        ))
        .expect("a narrow window");
        app.wait_for("label:Markdown source")
            .expect("the source editor");
        // A document opens in Source; the user picks Split.
        app.press("id:title-settings")
            .expect("open the glyph's menu");
        app.press("id:title-menu-split").expect("choose Split");
        app.wait_for("id:preview-field").expect("the field renders");
        app.wait_for("text:Release plan")
            .expect("the document renders");
        let shown = texts(&app);
        assert!(shown.contains("Preview"), "{}", dump(&app));
        assert!(shown.contains("Release plan"), "{}", dump(&app));
        // The preview is on screen, not just in the tree: its scroll has room
        // and the rendered heading lies inside it, in the narrow window's
        // overlay and at the wide breakpoint alike.
        assert_preview_is_visible(&app);
        app.set_viewport(nsh_core::Size::new(
            nsh_core::Points(1440.0),
            nsh_core::Points(900.0),
        ))
        .expect("a 1440x900 window");
        app.settle();
        assert_preview_is_visible(&app);
    }

    /// The field's preview scroll has a visible height, and the document's
    /// rendered heading starts inside it: a scroll laid out at zero height
    /// clips the whole document away.
    fn assert_preview_is_visible(app: &Headless) {
        let nodes = app.screen().layout_nodes();
        let scroll = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("preview-text"))
            .unwrap_or_else(|| panic!("the preview scroll: {}", dump(app)))
            .bounds;
        assert!(
            scroll.size.height.0 > 100.0,
            "the preview scroll is {}pt tall: {}",
            scroll.size.height.0,
            dump(app)
        );
        let heading = nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text.as_str() == "Release plan")
            })
            .unwrap_or_else(|| panic!("the rendered heading: {}", dump(app)))
            .bounds;
        let top = scroll.origin.y.0;
        let bottom = top + scroll.size.height.0;
        assert!(
            heading.origin.y.0 >= top && heading.origin.y.0 < bottom,
            "the heading at y{} is outside the preview scroll y{top}..{bottom}: {}",
            heading.origin.y.0,
            dump(app)
        );
    }
}
