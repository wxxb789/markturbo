//! The WebView compatibility path.
//!
//! The native renderer is the fast path; this is where browser semantics are
//! genuinely required: MDX component rendering, arbitrary HTML, Mermaid's
//! browser ecosystem, and MathML.
//!
//! Content is served to the WebView as a self-contained `data:` URL. That is
//! deliberate: a `data:` document has an opaque origin, so it cannot read local
//! files, cannot reach `file://`, and has no ambient credentials. Combined with
//! a restrictive CSP, this is the v0.1 trust boundary — untrusted MDX cannot
//! silently exfiltrate the workspace.

use mt_doc::{Block, BlockKind, Document};

use crate::renderer::{RenderOutcome, RendererRegistry};

/// How much the WebView is allowed to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// No scripting at all. Markdown and HTML render; MDX components do not.
    /// This is the default for any document, because a Markdown file in a
    /// cloned repository is not automatically trustworthy.
    Restricted,
    /// Scripts from the bundled runtime may run, enabling MDX components and
    /// browser-side diagram rendering. Requires an explicit user action.
    Trusted,
}

impl Trust {
    pub fn label(self) -> &'static str {
        match self {
            Trust::Restricted => "Restricted",
            Trust::Trusted => "Trusted",
        }
    }

    pub fn allows_scripts(self) -> bool {
        self == Trust::Trusted
    }
}

/// Build the HTML document shown in the WebView.
///
/// Diagrams and math are pre-rendered by the same registry the native path
/// uses, so both renderers are driven by one document model — the WebView is
/// not a second, divergent pipeline.
///
/// Uses the system color scheme. [`build_html_themed`] is the one the app
/// calls; this is for tests and any caller with no preset in hand.
pub fn build_html(doc: &Document, registry: &RendererRegistry, trust: Trust) -> String {
    build_html_themed(doc, registry, trust, None)
}

/// Build the HTML document, painted with a preset.
///
/// `None` leaves the document following the OS, which is right only when the
/// app is too. Passing the app's own preset is what keeps Split mode from
/// showing the same document in two palettes.
pub fn build_html_themed(
    doc: &Document,
    registry: &RendererRegistry,
    trust: Trust,
    preset: Option<&crate::theme::Preset>,
) -> String {
    let body = match doc.doc_type() {
        mt_doc::DocType::Mdx => render_mdx_body(doc, registry, trust),
        _ => render_markdown_body(doc, registry),
    };

    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n{csp}\n<style>{vars}\n{style}</style>\n</head><body>{banner}{body}</body></html>",
        csp = csp_meta(trust),
        vars = css_variables(preset),
        style = STYLE,
        banner = trust_banner(doc, trust),
    )
}

/// The `:root` block: the preset's palette as CSS custom properties.
///
/// Variable names follow ColaMD's, which is what the palettes were authored
/// against — so a preset transcribed from one of its stylesheets keeps meaning
/// the same thing here.
fn css_variables(preset: Option<&crate::theme::Preset>) -> String {
    let Some(preset) = preset else {
        // No preset: follow the OS, and let the stylesheet's `light-dark()`
        // values resolve. Every `var()` below has a fallback for exactly this.
        return ":root { color-scheme: light dark; }".to_string();
    };
    let t = preset.tokens;
    let hex = |c: u32| format!("#{c:06x}");
    format!(
        ":root {{\n  color-scheme: {scheme};\n  --bg-color: {bg};\n  --text-color: {text};\n  \
         --text-secondary: {secondary};\n  --text-muted: {muted};\n  --border-color: {border};\n  \
         --link-color: {link};\n  --code-bg: {code_bg};\n  --code-block-bg: {code_block_bg};\n  \
         --blockquote-border: {quote};\n  --table-header-bg: {table_head};\n  \
         --selection-bg: {selection};\n  --highlight-bg: {highlight};\n  --accent-color: {accent};\n  \
         --body-font: {font};\n  --body-line-height: {line_height};\n}}",
        scheme = if preset.dark { "dark" } else { "light" },
        bg = hex(t.bg),
        text = hex(t.text),
        secondary = hex(t.text_secondary),
        muted = hex(t.text_muted),
        border = hex(t.border),
        link = hex(t.link),
        code_bg = hex(t.code_bg),
        code_block_bg = hex(t.code_block_bg),
        quote = hex(t.blockquote_border),
        table_head = hex(t.table_header_bg),
        selection = hex(t.selection),
        highlight = hex(t.highlight),
        accent = hex(t.accent),
        font = preset.font.css(),
        line_height = preset.font.line_height(),
    )
}

/// The Content-Security-Policy for this trust level.
///
/// Both levels forbid every network fetch (`default-src 'none'`), so a document
/// can never phone home or leak what the user is reading. Restricted
/// additionally forbids script execution.
fn csp_meta(trust: Trust) -> String {
    let script = if trust.allows_scripts() {
        // 'unsafe-inline' is required because the MDX runtime is inlined into
        // the document; there is no server to issue a nonce. It is scoped to a
        // document the user explicitly trusted, and `default-src 'none'` still
        // blocks all network access.
        "script-src 'unsafe-inline'"
    } else {
        "script-src 'none'"
    };
    format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; {script}\">"
    )
}

fn trust_banner(doc: &Document, trust: Trust) -> String {
    // Only MDX can execute anything, so only MDX needs the warning.
    if doc.doc_type() != mt_doc::DocType::Mdx || trust.allows_scripts() {
        return String::new();
    }
    "<div class=\"mt-banner\">This MDX document is not trusted. Components are shown as placeholders and no code runs. Use <b>Trust this document</b> to enable full rendering.</div>".to_string()
}

/// Render an ordinary Markdown document to HTML.
fn render_markdown_body(doc: &Document, registry: &RendererRegistry) -> String {
    let mut out = String::new();
    let mut markdown_run = String::new();

    for block in doc.blocks() {
        match block.renderer_id() {
            Some(id) => {
                // Flush accumulated prose before the out-of-band block, so
                // ordering is preserved.
                flush_markdown(&mut out, &mut markdown_run, doc);
                out.push_str(&render_out_of_band(block, id, registry));
            }
            None => {
                markdown_run.push_str(&doc.source()[block.range.clone()]);
                markdown_run.push_str("\n\n");
            }
        }
    }
    flush_markdown(&mut out, &mut markdown_run, doc);
    out
}

fn flush_markdown(out: &mut String, run: &mut String, doc: &Document) {
    if run.trim().is_empty() {
        run.clear();
        return;
    }
    let options = mt_doc::doc::parse_options(doc.doc_type());
    match markdown::to_html_with_options(
        run,
        &markdown::Options {
            parse: options,
            compile: markdown::CompileOptions {
                // The source is a local file the user opened; raw HTML in it is
                // authored content, and blocking it would break ordinary
                // documents. Scripts are still blocked by the CSP.
                allow_dangerous_html: true,
                allow_dangerous_protocol: false,
                ..markdown::CompileOptions::gfm()
            },
        },
    ) {
        Ok(html) => out.push_str(&html),
        Err(message) => out.push_str(&diagnostic_html("markdown", &message.reason, run)),
    }
    run.clear();
}

/// Render a diagram/math block, or a diagnostic with the source preserved.
fn render_out_of_band(block: &Block, id: &str, registry: &RendererRegistry) -> String {
    match registry.render(id, &block.content) {
        RenderOutcome::Svg(markup) => {
            format!("<figure class=\"mt-render mt-{id}\">{markup}</figure>")
        }
        RenderOutcome::Failed(diag) => diagnostic_html(&diag.source, &diag.message, &block.content),
    }
}

/// Render an MDX document.
///
/// v0.1 does not embed a JS engine, so MDX components are shown as native-style
/// placeholders in both trust levels; what `Trusted` changes today is the CSP,
/// which is the boundary that has to be right before any runtime ships. The
/// Markdown parts of the document render fully either way.
fn render_mdx_body(doc: &Document, registry: &RendererRegistry, trust: Trust) -> String {
    let mut out = String::new();
    let mut markdown_run = String::new();

    for block in doc.blocks() {
        if let Some(id) = block.renderer_id() {
            flush_markdown(&mut out, &mut markdown_run, doc);
            out.push_str(&render_out_of_band(block, id, registry));
            continue;
        }
        match block.kind {
            BlockKind::Mdx(kind) => {
                flush_markdown(&mut out, &mut markdown_run, doc);
                out.push_str(&mdx_placeholder(kind, &block.content, trust));
            }
            _ => {
                markdown_run.push_str(&doc.source()[block.range.clone()]);
                markdown_run.push_str("\n\n");
            }
        }
    }
    flush_markdown(&mut out, &mut markdown_run, doc);

    for diag in doc.diagnostics().iter().filter(|d| d.source == "mdx") {
        out.push_str(&diagnostic_html(&diag.source, &diag.message, ""));
    }
    out
}

fn mdx_placeholder(kind: mt_doc::block::MdxKind, source: &str, _trust: Trust) -> String {
    let label = match kind {
        mt_doc::block::MdxKind::JsxElement => mt_doc::outline::jsx_tag_name(source)
            .map(|n| format!("&lt;{n} /&gt;"))
            .unwrap_or_else(|| "JSX".into()),
        other => other.label().to_string(),
    };
    format!(
        "<div class=\"mt-mdx\"><span class=\"mt-mdx-label\">{label}</span><pre>{}</pre></div>",
        escape(source)
    )
}

/// An inline diagnostic that always keeps the original source visible.
fn diagnostic_html(source: &str, message: &str, original: &str) -> String {
    let original = if original.trim().is_empty() {
        String::new()
    } else {
        format!("<pre>{}</pre>", escape(original))
    };
    format!(
        "<div class=\"mt-error\"><div class=\"mt-error-title\">{} rendering failed</div><div class=\"mt-error-msg\">{}</div>{original}</div>",
        escape(source),
        escape(message)
    )
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Encode `html` as a `data:` URL for `WebView::load_url`.
///
/// Percent-encoding rather than base64 keeps the payload debuggable and avoids
/// pulling in an encoder. The opaque origin of a `data:` URL is the point: it
/// has no filesystem or same-origin access to anything in the workspace.
pub fn to_data_url(html: &str) -> String {
    let mut out = String::with_capacity(html.len() * 2 + 32);
    out.push_str("data:text/html;charset=utf-8,");
    for byte in html.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The document stylesheet.
///
/// Every color reads a custom property with a fallback, so the same sheet works
/// both when a preset supplied `:root` variables and when it did not (the
/// system-following case, where the fallbacks are translucent grays that read on
/// either page).
const STYLE: &str = r#"
body { font-family: var(--body-font, -apple-system, "Segoe UI", system-ui, sans-serif);
       line-height: var(--body-line-height, 1.6);
       color: var(--text-color, inherit); background: var(--bg-color, transparent);
       margin: 0; padding: 32px 40px; max-width: 62rem; }
::selection { background: var(--selection-bg, rgba(127,127,127,.3)); }
a { color: var(--link-color, inherit); }
h1, h2, h3, h4, h5, h6 { line-height: 1.3; margin: 1.6em 0 .6em; }
h1 { font-size: 1.9em; letter-spacing: -0.01em; }
h1, h2 { border-bottom: 1px solid var(--border-color, rgba(127,127,127,.35));
         padding-bottom: .3em; }
strong { color: var(--accent-color, inherit); }
hr { border: none; border-top: 1px solid var(--border-color, rgba(127,127,127,.35));
     margin: 2em 0; }
pre  { background: var(--code-block-bg, rgba(127,127,127,.12)); padding: 14px 16px;
       border-radius: 8px; overflow-x: auto; line-height: 1.5; }
code { font-family: ui-monospace, "Cascadia Code", Consolas, monospace; font-size: .9em;
       background: var(--code-bg, rgba(127,127,127,.15)); border-radius: 4px;
       padding: .15em .35em; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; margin: 1.2em 0; }
th, td { border: 1px solid var(--border-color, rgba(127,127,127,.35)); padding: 7px 12px; }
th { background: var(--table-header-bg, rgba(127,127,127,.12)); }
mark { background: var(--highlight-bg, rgba(240,200,60,.35)); color: inherit; }
blockquote { border-left: 3px solid var(--blockquote-border, rgba(127,127,127,.4));
             color: var(--text-secondary, inherit); margin-left: 0; padding-left: 1.1em; }
img, svg { max-width: 100%; }
.mt-render { margin: 1.4em 0; text-align: center; }
.mt-error { border: 1px solid #d9534f; border-left-width: 4px; border-radius: 8px;
            padding: 10px 14px; margin: 1.2em 0; }
.mt-error-title { font-weight: 600; color: #d9534f; }
.mt-error-msg   { white-space: pre-wrap; margin: .4em 0; font-size: .92em; }
.mt-mdx { border: 1px dashed var(--border-color, rgba(127,127,127,.6)); border-radius: 8px;
          padding: 8px 12px; margin: 1.2em 0; }
.mt-mdx-label { font-family: ui-monospace, monospace; font-size: .85em;
                color: var(--text-muted, inherit); }
.mt-banner { background: rgba(240,173,78,.18); border: 1px solid rgba(240,173,78,.6);
             border-radius: 8px; padding: 10px 14px; margin-bottom: 1.5em; font-size: .92em; }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use mt_doc::DocType;

    fn registry() -> RendererRegistry {
        RendererRegistry::with_defaults()
    }

    /// The `<body>` content only. The stylesheet names every CSS class, so a
    /// whole-document `contains` check would match markup that never rendered.
    fn body(html: &str) -> &str {
        let start = html.find("<body>").expect("body") + "<body>".len();
        let end = html.rfind("</body>").expect("/body");
        &html[start..end]
    }

    fn md(src: &str) -> Document {
        Document::with_type(DocType::Markdown, src.to_string())
    }

    #[test]
    fn renders_ordinary_markdown() {
        let html = build_html(
            &md("# Title\n\nSome *text*.\n"),
            &registry(),
            Trust::Restricted,
        );
        assert!(html.contains("<h1>Title</h1>"), "got {html}");
        assert!(html.contains("<em>text</em>"));
    }

    #[test]
    fn restricted_csp_blocks_scripts_and_network() {
        let html = build_html(&md("# x\n"), &registry(), Trust::Restricted);
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("script-src 'none'"));
    }

    #[test]
    fn trusted_allows_scripts_but_still_blocks_network() {
        let html = build_html(&md("# x\n"), &registry(), Trust::Trusted);
        assert!(html.contains("script-src 'unsafe-inline'"));
        assert!(
            html.contains("default-src 'none'"),
            "trusted must not open network access"
        );
    }

    #[test]
    fn math_renders_to_svg() {
        let html = build_html(
            &md("$$\n\\frac{a}{b}\n$$\n"),
            &registry(),
            Trust::Restricted,
        );
        assert!(body(&html).contains("<svg"), "got {}", body(&html));
    }

    #[test]
    fn failed_render_shows_a_diagnostic_and_keeps_the_source() {
        // `d2` is almost certainly not installed in CI; either way the source
        // must survive and no panic may occur.
        let src = "```d2\nthis is the original source\n```\n";
        let html = build_html(&md(src), &registry(), Trust::Restricted);
        if body(&html).contains("mt-error") {
            assert!(
                html.contains("this is the original source"),
                "source must be preserved on failure"
            );
        } else {
            assert!(html.contains("<svg"), "either it rendered or it diagnosed");
        }
    }

    #[test]
    fn mdx_shows_component_placeholders_and_an_untrusted_banner() {
        let src = "# Title\n\n<RevenueChart data={rows} />\n";
        let doc = Document::with_type(DocType::Mdx, src.to_string());
        let html = build_html(&doc, &registry(), Trust::Restricted);
        assert!(
            body(&html).contains("mt-banner"),
            "untrusted MDX must be flagged"
        );
        assert!(html.contains("&lt;RevenueChart /&gt;"), "got {html}");
        assert!(
            html.contains("<h1>Title</h1>"),
            "markdown parts still render"
        );
    }

    #[test]
    fn trusted_mdx_has_no_banner() {
        let doc = Document::with_type(DocType::Mdx, "<Chart />\n".to_string());
        let html = build_html(&doc, &registry(), Trust::Trusted);
        assert!(!body(&html).contains("mt-banner"));
    }

    #[test]
    fn plain_markdown_never_shows_the_mdx_banner() {
        let html = build_html(&md("# x\n"), &registry(), Trust::Restricted);
        assert!(!body(&html).contains("mt-banner"));
    }

    #[test]
    fn escapes_html_in_diagnostics() {
        let out = diagnostic_html("test", "<script>alert(1)</script>", "<img onerror=x>");
        assert!(!out.contains("<script>"), "got {out}");
        assert!(out.contains("&lt;script&gt;"));
        assert!(out.contains("&lt;img onerror=x&gt;"));
    }

    #[test]
    fn data_url_round_trips_unicode() {
        let url = to_data_url("<p>中文 & <b>emoji 🎉</b></p>");
        assert!(url.starts_with("data:text/html;charset=utf-8,"));
        let encoded = &url["data:text/html;charset=utf-8,".len()..];
        // Decode and compare.
        let mut bytes = Vec::new();
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                let hex: String = chars.by_ref().take(2).collect();
                bytes.push(u8::from_str_radix(&hex, 16).unwrap());
            } else {
                bytes.push(c as u8);
            }
        }
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "<p>中文 & <b>emoji 🎉</b></p>"
        );
    }

    #[test]
    fn data_url_contains_no_unescaped_structural_characters() {
        // A `#` would truncate the document at a fragment; a quote or space
        // could break out of the URL in whatever context it is used. Encoding
        // everything outside the unreserved set is what makes the opaque-origin
        // sandbox actually hold.
        let html = build_html(
            &md("# Title\n\nText with \"quotes\", <tags>, #hashes & ampersands.\n"),
            &registry(),
            Trust::Restricted,
        );
        let url = to_data_url(&html);
        for bad in ['"', '\'', '<', '>', '#', '&', ' ', '\n', '\r'] {
            assert!(
                !url.contains(bad),
                "unescaped {bad:?} in the data URL would let content escape it"
            );
        }
    }

    #[test]
    fn cjk_and_tables_survive() {
        let src = "# 标题\n\n| 列一 | 列二 |\n|---|---|\n| 值 | 值 |\n";
        let html = build_html(&md(src), &registry(), Trust::Restricted);
        assert!(html.contains("标题"));
        assert!(html.contains("<table>"), "got {html}");
    }

    #[test]
    fn the_color_scheme_follows_the_preset() {
        let doc = md("# x\n");
        // No preset: follow the OS, which is right only when the app does too.
        assert!(
            build_html(&doc, &registry(), Trust::Restricted).contains("color-scheme: light dark")
        );

        for (id, dark, expected) in [
            ("light", false, "color-scheme: light"),
            ("dark", true, "color-scheme: dark"),
        ] {
            let preset = crate::theme::by_id(id, dark);
            let html = build_html_themed(&doc, &registry(), Trust::Restricted, Some(preset));
            assert!(html.contains(expected), "{id} produced {html}");
            // A pinned scheme must not also leave the paired value in, or the
            // browser falls back to following the OS.
            assert!(
                !html.contains("color-scheme: light dark"),
                "{id} must pin the scheme"
            );
        }
    }

    #[test]
    fn a_preset_paints_the_document_with_its_own_palette() {
        // The whole point of the preset reaching the WebView: the preview and
        // the chrome around it must not disagree about what Nord looks like.
        let preset = crate::theme::by_id("nord", true);
        let html = build_html_themed(&md("# x\n"), &registry(), Trust::Restricted, Some(preset));
        assert!(
            html.contains(&format!("--bg-color: #{:06x}", preset.tokens.bg)),
            "got {html}"
        );
        assert!(html.contains(&format!("--link-color: #{:06x}", preset.tokens.link)));
        // The stylesheet has to actually consume them, or the variables are
        // decoration.
        assert!(html.contains("var(--bg-color"));
        assert!(html.contains("var(--link-color"));
    }

    #[test]
    fn a_reading_preset_carries_its_typeface_into_the_preview() {
        let writer = crate::theme::by_id("writer", false);
        let html = build_html_themed(&md("# x\n"), &registry(), Trust::Restricted, Some(writer));
        assert!(html.contains("--body-font:"), "got {html}");
        assert!(html.contains("monospace"), "Writer is a monospace preset");
        assert!(html.contains("var(--body-font"));
    }

    #[test]
    fn theming_does_not_weaken_the_csp() {
        // The palette is injected next to the stylesheet; a mistake there would
        // be the kind that quietly drops a directive.
        let html = build_html_themed(
            &md("# x\n"),
            &registry(),
            Trust::Restricted,
            Some(crate::theme::by_id("dracula", true)),
        );
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("script-src 'none'"));
    }

    #[test]
    fn block_order_is_preserved_around_diagrams() {
        let src = "First.\n\n$$\nx\n$$\n\nLast.\n";
        let html = build_html(&md(src), &registry(), Trust::Restricted);
        let body = body(&html);
        let first = body.find("First.").expect("first paragraph");
        let math = body.find("mt-render").expect("rendered math");
        let last = body.find("Last.").expect("last paragraph");
        assert!(
            first < math && math < last,
            "blocks out of order in:
{body}"
        );
    }
}
