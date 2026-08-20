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
pub fn build_html(doc: &Document, registry: &RendererRegistry, trust: Trust) -> String {
    let body = match doc.doc_type() {
        mt_doc::DocType::Mdx => render_mdx_body(doc, registry, trust),
        _ => render_markdown_body(doc, registry),
    };

    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n{csp}\n<style>{style}</style>\n</head><body>{banner}{body}</body></html>",
        csp = csp_meta(trust),
        style = STYLE,
        banner = trust_banner(doc, trust),
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
        RenderOutcome::Failed(diag) => {
            diagnostic_html(&diag.source, &diag.message, &block.content)
        }
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

const STYLE: &str = r#"
:root { color-scheme: light dark; }
body { font-family: -apple-system, "Segoe UI", system-ui, sans-serif; line-height: 1.6;
       margin: 0; padding: 24px 32px; max-width: 60rem; }
pre  { background: rgba(127,127,127,.12); padding: 12px; border-radius: 6px; overflow-x: auto; }
code { font-family: ui-monospace, "Cascadia Code", Consolas, monospace; font-size: .9em; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; }
th, td { border: 1px solid rgba(127,127,127,.35); padding: 6px 10px; }
blockquote { border-left: 3px solid rgba(127,127,127,.4); margin-left: 0; padding-left: 1em;
             opacity: .85; }
img, svg { max-width: 100%; }
.mt-render { margin: 1em 0; text-align: center; }
.mt-error { border: 1px solid #d9534f; border-left-width: 4px; border-radius: 6px;
            padding: 10px 14px; margin: 1em 0; }
.mt-error-title { font-weight: 600; color: #d9534f; }
.mt-error-msg   { white-space: pre-wrap; margin: .4em 0; font-size: .92em; }
.mt-mdx { border: 1px dashed rgba(127,127,127,.6); border-radius: 6px; padding: 8px 12px;
          margin: 1em 0; }
.mt-mdx-label { font-family: ui-monospace, monospace; font-size: .85em; opacity: .75; }
.mt-banner { background: rgba(240,173,78,.18); border: 1px solid rgba(240,173,78,.6);
             border-radius: 6px; padding: 10px 14px; margin-bottom: 1.5em; font-size: .92em; }
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
        let html = build_html(&md("# Title\n\nSome *text*.\n"), &registry(), Trust::Restricted);
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
        let html = build_html(&md("$$\n\\frac{a}{b}\n$$\n"), &registry(), Trust::Restricted);
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
        assert!(body(&html).contains("mt-banner"), "untrusted MDX must be flagged");
        assert!(html.contains("&lt;RevenueChart /&gt;"), "got {html}");
        assert!(html.contains("<h1>Title</h1>"), "markdown parts still render");
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
    fn block_order_is_preserved_around_diagrams() {
        let src = "First.\n\n$$\nx\n$$\n\nLast.\n";
        let html = build_html(&md(src), &registry(), Trust::Restricted);
        let body = body(&html);
        let first = body.find("First.").expect("first paragraph");
        let math = body.find("mt-render").expect("rendered math");
        let last = body.find("Last.").expect("last paragraph");
        assert!(first < math && math < last, "blocks out of order in:
{body}");
    }
}
