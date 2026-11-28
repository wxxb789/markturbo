use std::borrow::Cow;
use std::collections::HashMap;

use mt_core::rendering::{RenderOutcome, RendererRegistry};
use mt_core::settings::Language;
use mt_core::{Block, BlockKind, DocType, Document};
use nsh_core::Color;

const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:";
const HOST_FRAME_SRC: &str = "; frame-src 'self'";
// WebView2 NavigateToString limits the UTF-16 htmlContent to 2 * 1024 * 1024 bytes.
const MAX_HOST_UTF16_UNITS: usize = 1024 * 1024;
const FRAME_PRELUDE: &str = r#"<meta http-equiv="x-dns-prefetch-control" content="off"><script>(() => {
    for (const name of [
        "RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel",
        "RTCIceTransport", "RTCIceCandidate", "RTCRtpSender", "RTCRtpReceiver",
        "RTCRtpTransceiver", "RTCSctpTransport", "RTCDtlsTransport", "RTCSessionDescription"
    ]) {
        const disabled = {value: undefined, configurable: false, writable: false};
        for (let proto = Object.getPrototypeOf(window); proto; proto = Object.getPrototypeOf(proto)) {
            if (Object.getOwnPropertyDescriptor(proto, name)) Object.defineProperty(proto, name, disabled);
        }
        Object.defineProperty(window, name, disabled);
    }
    const linkUrl = event => {
        const link = event.composedPath().find(node =>
            (node.localName === "a" || node.localName === "area") &&
            (node.hasAttribute("href") || node.hasAttribute("xlink:href")));
        if (!link) return null;
        event.preventDefault();
        const href = link.getAttribute("href") ?? link.getAttribute("xlink:href") ?? "";
        const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(href);
        return scheme && ["http", "https", "mailto"].includes(scheme[1].toLowerCase()) ? href : null;
    };
    const followLink = event => {
        const url = linkUrl(event);
        if (url !== null) parent.postMessage(url, "*");
    };
    addEventListener("click", followLink, true);
    addEventListener("auxclick", followLink, true);
    addEventListener("submit", event => event.preventDefault(), true);
})();</script>"#;

/// The document frame cannot reach the session bridge on every backend
/// (`nsh-platform-linux` and `nsh-platform-macos` inject it into the top
/// frame only), so the host carries the frame's URL message to the bridge.
const HOST_RELAY: &str = "<script>(() => {
    const frame = document.querySelector(\"iframe\");
    addEventListener(\"message\", event => {
        if (event.source !== frame.contentWindow || typeof event.data !== \"string\") return;
        if (window.nsh && typeof window.nsh.postMessage === \"function\") window.nsh.postMessage(event.data);
    });
})();</script>";

pub fn build_html(doc: &Document, registry: &RendererRegistry, dark: bool) -> String {
    build_html_localized(doc, registry, dark, Language::default())
}

pub(crate) fn build_html_localized(
    doc: &Document,
    registry: &RendererRegistry,
    dark: bool,
    language: Language,
) -> String {
    let head_prefix = format!("{}{FRAME_PRELUDE}", csp_meta(""));
    let page = match doc.doc_type() {
        DocType::Html => build_html_raw(doc.source(), &head_prefix),
        kind => {
            let body = if kind == DocType::Mdx {
                render_mdx_body(doc, registry, language)
            } else {
                render_markdown_body(doc, registry, language)
            };
            format!(
                "<!doctype html><html><head>{head_prefix}<meta charset=\"utf-8\">\
                 <style>{vars}\n:root {{\n  --danger-color: {danger};\n}}\n{STYLE}</style>\
                 </head><body>{body}</body></html>",
                vars = css_variables(dark),
                danger = css_hex_color(if dark {
                    crate::ui::tokens::DARK.danger
                } else {
                    crate::ui::tokens::LIGHT.danger
                }),
            )
        }
    };
    let host_prefix = format!(
        "<!doctype html><html><head>{host_csp}<meta charset=\"utf-8\">\
         <style>iframe{{position:fixed;inset:0;width:100%;height:100%;border:0}}</style>\
         </head><body><iframe sandbox=\"allow-scripts\" srcdoc=\"",
        host_csp = csp_meta(HOST_FRAME_SRC),
    );
    let host_suffix = format!("\"></iframe>{HOST_RELAY}</body></html>");
    let page = neutralize_document_links(&page);
    let host = format!("{host_prefix}{}{host_suffix}", escape_srcdoc(&page));
    if host.encode_utf16().count() > MAX_HOST_UTF16_UNITS {
        return format!(
            "<!doctype html><html><head>{}<meta charset=\"utf-8\"></head>\
             <body><p role=\"alert\">This preview is too large to display.</p></body></html>",
            csp_meta("")
        );
    }
    host
}

#[derive(Clone, Copy)]
enum LinkScope {
    Document,
    ForeignObject(usize),
    Foreign(&'static str),
}

fn neutralize_document_links(page: &str) -> Cow<'_, str> {
    let mut remaining = page;
    let mut output = None;
    let mut copied = 0;
    let mut scopes = vec![LinkScope::Document];
    let mut tag_tail_len = None;
    while let Some((_, after_start)) = remaining.split_once('<') {
        remaining = after_start;
        let Some(attributes) = tag_attributes(after_start, "link") else {
            if tag_tail_len.is_some_and(|len| after_start.len() > len) {
                continue;
            }
            if let Some(comment) = after_start.strip_prefix("!--") {
                remaining = comment_end_tail(comment).unwrap_or_default();
                continue;
            }
            let (closing, tag) = match after_start.strip_prefix('/') {
                Some(tag) => (true, tag),
                None => (false, after_start),
            };
            if !tag.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
                continue;
            }
            let name_end = tag
                .find(|c| html_whitespace(c) || matches!(c, '/' | '>'))
                .unwrap_or(tag.len());
            let (name, attributes) = tag.split_at(name_end);
            let end = scan_tag_end(attributes, |_| {});
            let after_tag = attributes.get(end.offset + 1..).unwrap_or_default();
            tag_tail_len = Some(after_tag.len());
            match scopes.last().copied() {
                Some(LinkScope::Foreign(root)) => {
                    if closing && name.eq_ignore_ascii_case(root) {
                        scopes.pop();
                    } else if !closing && !end.slash {
                        if root == "svg" && name.eq_ignore_ascii_case("foreignObject") {
                            scopes.push(LinkScope::ForeignObject(0));
                        } else if let Some(next) = ["svg", "math"]
                            .into_iter()
                            .find(|next| name.eq_ignore_ascii_case(next))
                        {
                            scopes.push(LinkScope::Foreign(next));
                        }
                    }
                }
                _ => {
                    if closing {
                        if name.eq_ignore_ascii_case("foreignObject") {
                            let pop = match scopes.last_mut() {
                                Some(LinkScope::ForeignObject(depth)) if *depth == 0 => true,
                                Some(LinkScope::ForeignObject(depth)) => {
                                    *depth -= 1;
                                    false
                                }
                                _ => false,
                            };
                            if pop {
                                scopes.pop();
                            }
                        }
                    } else if name.eq_ignore_ascii_case("plaintext") {
                        break;
                    } else if is_html_raw_text_name(name) {
                        remaining = raw_text_end_tail(after_tag, name).unwrap_or_default();
                    } else if !end.slash
                        && let Some(root) = ["svg", "math"]
                            .into_iter()
                            .find(|root| name.eq_ignore_ascii_case(root))
                    {
                        scopes.push(LinkScope::Foreign(root));
                    } else if !end.slash
                        && name.eq_ignore_ascii_case("foreignObject")
                        && let Some(LinkScope::ForeignObject(depth)) = scopes.last_mut()
                    {
                        *depth += 1;
                    }
                }
            }
            continue;
        };
        let start = page.len() - after_start.len() - 1;
        let neutralized = output.get_or_insert_with(|| String::with_capacity(page.len()));
        neutralized.push_str(&page[copied..start]);
        neutralized.push_str("<template data-mt-link");
        let mut copied_attributes = 0;
        let end = scan_tag_end(attributes, |offset| {
            if tag_attributes(&attributes[offset + 1..], "link").is_some() {
                neutralized.push_str(&attributes[copied_attributes..offset]);
                neutralized.push_str("&lt;");
                copied_attributes = offset + 1;
            }
        });
        neutralized.push_str(&attributes[copied_attributes..end.offset - usize::from(end.slash)]);
        if let Some(quote) = end.quote {
            neutralized.push(quote.into());
        }
        neutralized.push_str("></template>");
        remaining = attributes.get(end.offset + 1..).unwrap_or_default();
        copied = page.len() - remaining.len();
    }
    match output {
        Some(mut neutralized) => {
            neutralized.push_str(&page[copied..]);
            Cow::Owned(neutralized)
        }
        None => Cow::Borrowed(page),
    }
}

fn comment_end_tail(comment: &str) -> Option<&str> {
    comment
        .strip_prefix('>')
        .or_else(|| comment.strip_prefix("->"))
        .or_else(|| {
            comment.match_indices('-').find_map(|(offset, _)| {
                let tail = &comment[offset + 1..];
                tail.strip_prefix("->").or_else(|| tail.strip_prefix("-!>"))
            })
        })
}

fn tag_attributes<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let (tag, rest) = source.split_at_checked(name.len())?;
    (tag.eq_ignore_ascii_case(name)
        && (rest.is_empty() || rest.starts_with(|c| html_whitespace(c) || matches!(c, '/' | '>'))))
    .then_some(rest)
}

struct TagEnd {
    offset: usize,
    slash: bool,
    quote: Option<u8>,
}

fn scan_tag_end(attributes: &str, mut visit_less_than: impl FnMut(usize)) -> TagEnd {
    #[derive(Clone, Copy)]
    enum State {
        BeforeName,
        Name,
        AfterName,
        BeforeValue,
        Unquoted,
        Quoted(u8),
        Slash,
    }
    use State::*;

    let mut state = BeforeName;
    for (offset, byte) in attributes.bytes().enumerate() {
        if byte == b'<' {
            visit_less_than(offset);
        }
        let space = html_whitespace(byte.into());
        state = match (state, byte) {
            (Quoted(quote), byte) if quote == byte => BeforeName,
            (Quoted(_), _) => state,
            (_, b'>') => {
                return TagEnd {
                    offset,
                    slash: matches!(state, Slash),
                    quote: None,
                };
            }
            (Unquoted, _) if space => BeforeName,
            (Unquoted, _) => Unquoted,
            (BeforeValue, b'\'' | b'"') => Quoted(byte),
            (BeforeValue, _) if space => BeforeValue,
            (BeforeValue, _) => Unquoted,
            (Name | AfterName, b'=') => BeforeValue,
            (Name | AfterName, _) if space => AfterName,
            (_, _) if space => BeforeName,
            (_, b'/') => Slash,
            _ => Name,
        };
    }
    TagEnd {
        offset: attributes.len(),
        slash: matches!(state, Slash),
        quote: match state {
            Quoted(quote) => Some(quote),
            _ => None,
        },
    }
}

fn escape_srcdoc(page: &str) -> String {
    let mut out = String::with_capacity(page.len());
    for c in page.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn csp_meta(extra: &str) -> String {
    format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}{extra}\">")
}

/// Preserve the original shell when its leading structure is unambiguous.
/// Otherwise let the HTML parser imply a head from the leading policy meta.
fn build_html_raw(source: &str, head_prefix: &str) -> String {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let start = skip_html_trivia(source);
    let (after_doctype, has_doctype) = match html_doctype_end(start) {
        Some(rest) => (rest, true),
        None => (start, false),
    };
    let before_html = skip_html_trivia(after_doctype);
    let after_html = opening_tag_end(before_html, "html").unwrap_or(before_html);
    let head = opening_tag_end(skip_html_trivia(after_html), "head");
    let (prefix, rest) = source.split_at(source.len() - head.unwrap_or(after_html).len());
    let doctype = if has_doctype { "" } else { "<!doctype html>" };
    format!("{doctype}{prefix}{head_prefix}{rest}")
}

fn html_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c')
}

const HTML_RAW_TEXT_NAMES: [&str; 10] = [
    "script",
    "style",
    "textarea",
    "title",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
];

fn raw_text_name(name: &str) -> Option<&'static str> {
    HTML_RAW_TEXT_NAMES
        .into_iter()
        .find(|raw| name.eq_ignore_ascii_case(raw))
}

fn is_html_raw_text_name(name: &str) -> bool {
    raw_text_name(name).is_some()
}

fn html_doctype_end(source: &str) -> Option<&str> {
    let keyword = source.get(..9)?;
    if !keyword.eq_ignore_ascii_case("<!doctype") {
        return None;
    }

    let mut rest = take_html_whitespace(&source[9..])?;
    let name_end = rest.find(|c: char| html_whitespace(c) || c == '>')?;
    if !rest[..name_end].eq_ignore_ascii_case("html") {
        return None;
    }
    rest = &rest[name_end..];

    if let Some(tail) = rest.strip_prefix('>') {
        return Some(tail);
    }
    rest = take_html_whitespace(rest)?;
    if let Some(tail) = rest.strip_prefix('>') {
        return Some(tail);
    }

    if let Some(after_public) = take_doctype_keyword(rest, "public") {
        let public_id = take_quoted_doctype_id(take_html_whitespace(after_public)?)?;
        let after_id = public_id.trim_start_matches(html_whitespace);
        let rest = if after_id.starts_with('>') {
            after_id
        } else if after_id.len() != public_id.len() {
            take_quoted_doctype_id(after_id)?
        } else {
            public_id
        };
        return rest.trim_start_matches(html_whitespace).strip_prefix('>');
    }

    if let Some(after_system) = take_doctype_keyword(rest, "system") {
        let system_id = take_quoted_doctype_id(take_html_whitespace(after_system)?)?;
        return system_id
            .trim_start_matches(html_whitespace)
            .strip_prefix('>');
    }

    None
}

fn take_html_whitespace(source: &str) -> Option<&str> {
    let rest = source.trim_start_matches(html_whitespace);
    (rest.len() != source.len()).then_some(rest)
}

fn take_doctype_keyword<'a>(source: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = source.get(..keyword.len())?;
    if !rest.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &source[keyword.len()..];
    rest.chars()
        .next()
        .is_some_and(html_whitespace)
        .then_some(rest)
}

fn take_quoted_doctype_id(source: &str) -> Option<&str> {
    let quote = source.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let end = source[quote.len_utf8()..].find(quote)? + quote.len_utf8();
    if source[quote.len_utf8()..end].contains('>') {
        return None;
    }
    Some(&source[end + quote.len_utf8()..])
}

fn skip_html_trivia(mut source: &str) -> &str {
    loop {
        source = source.trim_start_matches(html_whitespace);
        let Some(comment) = source.strip_prefix("<!--") else {
            return source;
        };
        let Some(rest) = html_comment_end(comment) else {
            return source;
        };
        source = rest;
    }
}

fn html_comment_end(comment: &str) -> Option<&str> {
    if comment.starts_with('>') || comment.starts_with("->") {
        return None;
    }
    let (body, rest) = comment.split_once("-->")?;
    if body.contains("--!>") {
        return None;
    }
    Some(rest)
}

fn opening_tag_end<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let source = source.strip_prefix('<')?;
    if !source.get(..name.len())?.eq_ignore_ascii_case(name) {
        return None;
    }
    let mut rest = &source[name.len()..];
    loop {
        if let Some(tail) = rest.strip_prefix('>') {
            return Some(tail);
        }
        let attrs = rest.trim_start_matches(html_whitespace);
        if attrs.len() == rest.len() {
            return None;
        }
        if let Some(tail) = attrs.strip_prefix('>') {
            return Some(tail);
        }
        let name_end = attrs
            .find(|c: char| {
                html_whitespace(c) || matches!(c, '=' | '>' | '/' | '<' | '\'' | '"' | '\0')
            })
            .unwrap_or(attrs.len());
        if name_end == 0 {
            return None;
        }
        if attrs[..name_end].eq_ignore_ascii_case("manifest") {
            return None;
        }
        rest = &attrs[name_end..];
        if let Some(value) = rest.trim_start_matches(html_whitespace).strip_prefix('=') {
            let value = value.trim_start_matches(html_whitespace);
            if let Some(quote @ ('\'' | '"')) = value.chars().next() {
                rest = value[1..].split_once(quote)?.1;
            } else {
                let end = value
                    .find(|c: char| {
                        html_whitespace(c) || matches!(c, '>' | '<' | '=' | '\'' | '"' | '`' | '\0')
                    })
                    .unwrap_or(value.len());
                if end == 0 {
                    return None;
                }
                rest = &value[end..];
            }
        }
    }
}

fn css_variables(dark: bool) -> &'static str {
    if dark {
        r#":root {
  color-scheme: dark;
  --bg-color: #0d1117;
  --text-color: #e6edf3;
  --text-secondary: #aab3bd;
  --text-muted: #8b949e;
  --border-color: #30363d;
  --link-color: #58a6ff;
  --code-bg: #2c333c;
  --code-block-bg: #161b22;
  --blockquote-border: #30363d;
  --table-header-bg: #161b22;
  --selection-bg: #1f3f66;
  --highlight-bg: #4a3a12;
  --accent-color: #58a6ff;
  --body-font: -apple-system, "Segoe UI", system-ui, "PingFang SC", "Noto Sans SC", sans-serif;
  --body-line-height: 1.6;
}"#
    } else {
        r#":root {
  color-scheme: light;
  --bg-color: #ffffff;
  --text-color: #24292f;
  --text-secondary: #656d76;
  --text-muted: #656d76;
  --border-color: #d0d7de;
  --link-color: #0969da;
  --code-bg: #eef1f4;
  --code-block-bg: #f6f8fa;
  --blockquote-border: #d0d7de;
  --table-header-bg: #f6f8fa;
  --selection-bg: #cfe3fb;
  --highlight-bg: #fff3bf;
  --accent-color: #0969da;
  --body-font: -apple-system, "Segoe UI", system-ui, "PingFang SC", "Noto Sans SC", sans-serif;
  --body-line-height: 1.6;
}"#
    }
}

fn css_hex_color(color: Color) -> String {
    let (red, green, blue, _) = color.to_f64();
    format!(
        "#{:02X}{:02X}{:02X}",
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8
    )
}

fn render_markdown_body(doc: &Document, registry: &RendererRegistry, language: Language) -> String {
    let mut out = String::new();
    let mut markdown_run = String::new();

    for block in doc.blocks() {
        match block.renderer_id() {
            Some(id) => {
                // Flush accumulated prose before the out-of-band block, so
                // ordering is preserved.
                flush_markdown(&mut out, &mut markdown_run, doc);
                out.push_str(&render_out_of_band(block, id, registry, language));
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
    let mut options = markdown::Options {
        parse: mt_core::document::doc::parse_options(doc.doc_type()),
        compile: markdown::CompileOptions {
            allow_dangerous_html: true,
            allow_dangerous_protocol: false,
            gfm_tagfilter: true,
            ..markdown::CompileOptions::default()
        },
    };
    let rendered = render_markdown_scripts(run, &mut options, |source, options| {
        markdown::to_html_with_options(source, options).map_err(|message| message.reason)
    });
    match rendered {
        Ok(html) => out.push_str(&html),
        Err(message) => out.push_str(&diagnostic_html("markdown rendering failed", &message, run)),
    }
    run.clear();
}

struct ScriptPlaceholder<'a> {
    marker: String,
    script: &'a str,
    before: &'static str,
    after: &'static str,
}

type ScriptPlaceholders<'a> = Vec<ScriptPlaceholder<'a>>;

struct ScriptRange {
    start: usize,
    end: usize,
    script: String,
    flow: bool,
}

fn render_markdown_scripts(
    source: &str,
    options: &mut markdown::Options,
    mut compile: impl FnMut(&str, &markdown::Options) -> Result<String, String>,
) -> Result<String, String> {
    let (mut ranges, mut jsx_names) = collect_script_ranges(source, &options.parse);
    let html_text = options.parse.constructs.html_text;
    ranges.sort_unstable_by_key(|range| range.start);
    jsx_names.sort_unstable();
    let mut end = 0;
    ranges.retain(|range| {
        if range.start < end {
            return false;
        }
        end = range.end;
        true
    });
    while !ranges.is_empty() {
        let (protected, scripts) = protect_script_ranges(
            source,
            &ranges,
            &jsx_names,
            options.parse.constructs.mdx_jsx_text,
        )?;
        options.parse.constructs.html_text = true;
        let html = compile(&protected, options)?;
        match restore_inline_scripts(html, scripts) {
            Ok(html) => return Ok(html),
            Err(restore_failed) => {
                let mut failures = restore_failed.into_iter();
                ranges.retain(|_| !failures.next().expect("one status per script"));
            }
        }
    }
    options.parse.constructs.html_text = html_text;
    compile(source, options)
}

fn collect_script_ranges(
    source: &str,
    parse_options: &markdown::ParseOptions,
) -> (Vec<ScriptRange>, Vec<usize>) {
    if !source
        .as_bytes()
        .windows(b"<script".len())
        .any(|tag| tag.eq_ignore_ascii_case(b"<script"))
    {
        return (Vec::new(), Vec::new());
    }

    let Ok(ast) = markdown::to_mdast(source, parse_options) else {
        return (Vec::new(), Vec::new());
    };
    let mut ranges = Vec::new();
    let mut jsx_names = Vec::new();
    let context = ScriptContext::default();
    let mut scope = RawTextScope::None;
    collect_inline_script_ranges(
        &ast,
        source,
        &mut ranges,
        &mut jsx_names,
        &context,
        &mut scope,
    );
    (ranges, jsx_names)
}

fn protect_script_ranges<'a>(
    source: &str,
    ranges: &'a [ScriptRange],
    jsx_names: &[usize],
    mdx: bool,
) -> Result<(String, ScriptPlaceholders<'a>), String> {
    let marker_prefix = random_script_marker_prefix()?;
    let mut protected = String::with_capacity(source.len());
    let mut scripts = Vec::with_capacity(ranges.len());
    let mut jsx_names = jsx_names.iter().copied().peekable();
    let mut cursor = 0;
    for (
        index,
        &ScriptRange {
            start,
            end,
            ref script,
            flow,
        },
    ) in ranges.iter().enumerate()
    {
        copy_mdx_text(&mut protected, source, cursor, start, &mut jsx_names);
        let marker = format!("{marker_prefix}:{index}-->");
        if flow && !mdx {
            let (close, after) = if script_close_ends_block(script) {
                (FLOW_CLOSE, FLOW_CLOSE_FILTERED)
            } else {
                (FLOW_CONTINUING_CLOSE, FLOW_CONTINUING_CLOSE_FILTERED)
            };
            protected.push_str(FLOW_OPEN);
            protected.push_str(&marker);
            protected.push_str(close);
            scripts.push(ScriptPlaceholder {
                marker,
                script,
                before: FLOW_OPEN_FILTERED,
                after,
            });
        } else {
            // A punctuation guard keeps a line-leading comment in MDX's inline grammar.
            let gap = &source[cursor..start];
            let guard = if mdx && (flow || starts_mdx_line(gap, cursor == 0)) {
                "."
            } else {
                ""
            };
            let marker = format!("{guard}{marker}");
            protected.push_str(&marker);
            scripts.push(ScriptPlaceholder {
                marker,
                script,
                before: "",
                after: "",
            });
        }
        cursor = end;
    }
    copy_mdx_text(&mut protected, source, cursor, source.len(), &mut jsx_names);
    Ok((protected, scripts))
}

const FLOW_OPEN: &str = "<script>";
const FLOW_CLOSE: &str = "</script>";
const FLOW_CONTINUING_CLOSE: &str = "</script >";
const FLOW_OPEN_FILTERED: &str = "&lt;script>";
const FLOW_CLOSE_FILTERED: &str = "&lt;/script>";
const FLOW_CONTINUING_CLOSE_FILTERED: &str = "&lt;/script >";

fn script_close_ends_block(script: &str) -> bool {
    script
        .get(script.len().saturating_sub(FLOW_CLOSE.len())..)
        .is_some_and(|close| close.eq_ignore_ascii_case(FLOW_CLOSE))
}

fn starts_mdx_line(gap: &str, first_script: bool) -> bool {
    gap.rsplit_once(['\r', '\n'])
        .map(|(_, line)| line)
        .or_else(|| first_script.then_some(gap))
        .is_some_and(|line| line.chars().all(|c| html_whitespace(c) || c == '>'))
}

fn copy_mdx_text(
    out: &mut String,
    source: &str,
    mut cursor: usize,
    end: usize,
    jsx_names: &mut std::iter::Peekable<impl Iterator<Item = usize>>,
) {
    while let Some(offset) = jsx_names.next_if(|&offset| offset < end) {
        out.push_str(&source[cursor..offset]);
        // A dollar-prefixed name stays an MDX tag when HTML text is enabled.
        if !source[offset..].starts_with(html_whitespace) {
            out.push('$');
        }
        cursor = offset;
    }
    out.push_str(&source[cursor..end]);
}

fn random_script_marker_prefix() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|error| format!("could not generate script placeholder nonce: {error}"))?;

    let mut prefix = String::from("<!--mt-script:");
    for byte in nonce {
        prefix.push(HEX[usize::from(byte >> 4)] as char);
        prefix.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    Ok(prefix)
}

#[derive(Default)]
struct ScriptContext<'a> {
    inline_start: Option<usize>,
    list_opener: Option<(ListOpener<'a>, &'a ScriptContext<'a>)>,
}

#[derive(Clone, Copy)]
struct ListOpener<'a> {
    line_start: usize,
    prefix: &'a str,
    has_content: bool,
}

impl<'a> ListOpener<'a> {
    fn new(source: &'a str, item: &markdown::mdast::ListItem) -> Option<Self> {
        let position = item.position.as_ref()?;
        let line_start = line_start(source, position.start.offset);
        let content = item
            .children
            .first()
            .and_then(|child| child.position())
            .filter(|child| child.start.line == position.start.line);
        let prefix = match content {
            Some(child) => &source[line_start..child.start.offset],
            None => source[line_start..].split(['\r', '\n']).next()?,
        };
        Some(Self {
            line_start,
            prefix,
            has_content: content.is_some(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawTextScope {
    None,
    Comment,
    Element(&'static str),
}

fn collect_inline_script_ranges(
    node: &markdown::mdast::Node,
    source: &str,
    ranges: &mut Vec<ScriptRange>,
    jsx_names: &mut Vec<usize>,
    context: &ScriptContext<'_>,
    scope: &mut RawTextScope,
) {
    use markdown::mdast::Node;
    let phrasing_block = matches!(
        node,
        Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_)
    );
    let next_context;
    let context = match node {
        Node::ListItem(item) => {
            next_context = ScriptContext {
                inline_start: None,
                list_opener: ListOpener::new(source, item).map(|opener| (opener, context)),
            };
            &next_context
        }
        _ if phrasing_block => {
            next_context = ScriptContext {
                inline_start: node.position().map(|position| position.start.offset),
                list_opener: context.list_opener,
            };
            &next_context
        }
        _ => context,
    };
    if context.inline_start.is_none()
        && let Node::Html(html) = node
    {
        collect_markdown_flow_scripts(html, source, ranges, scope);
        return;
    }
    let jsx = match node {
        Node::MdxJsxTextElement(jsx) => Some((&jsx.name, &jsx.position, true)),
        Node::MdxJsxFlowElement(jsx) => Some((&jsx.name, &jsx.position, false)),
        _ => None,
    };
    if let Some((name, Some(position), inline)) = jsx
        && let Some(raw) = source.get(position.start.offset..position.end.offset)
    {
        let start = position.start.offset;
        if name.as_deref() == Some("script") && !raw.ends_with("/>") {
            let end = position.end.offset;
            if let Some(script) = script_value(source, context, start, end) {
                ranges.push(ScriptRange {
                    start,
                    end,
                    script,
                    flow: !inline,
                });
            }
            return;
        }
        if inline {
            jsx_names.push(start + 1);
            if !raw.ends_with("/>")
                && let Some(close) = raw.rfind("</")
            {
                jsx_names.push(start + close + 2);
            }
        }
    }

    if phrasing_block {
        let mut events = Vec::new();
        collect_phrasing_events(node, source, &mut events);
        events.sort_unstable_by_key(|(offset, _)| *offset);

        let mut opening = None;
        for (_, event) in events {
            match event {
                PhrasingEvent::CommentOpen => {
                    if *scope == RawTextScope::None && opening.is_none() {
                        *scope = RawTextScope::Comment;
                    }
                }
                PhrasingEvent::CommentClose => {
                    if *scope == RawTextScope::Comment {
                        *scope = RawTextScope::None;
                    }
                }
                PhrasingEvent::Tag(start, end, tag) => {
                    if let RawTextScope::Element(name) = *scope {
                        if name != "plaintext" && is_html_end_tag(tag, name) {
                            *scope = RawTextScope::None;
                        }
                        continue;
                    }
                    if *scope == RawTextScope::Comment {
                        continue;
                    }
                    if opening.is_none()
                        && !is_script_tag(tag, false)
                        && let Some(name) = html_raw_text_name(tag)
                    {
                        *scope = RawTextScope::Element(name);
                        continue;
                    }
                    if is_script_tag(tag, false) {
                        opening.get_or_insert(start);
                    } else if is_script_tag(tag, true)
                        && let Some(start) = opening.take()
                        && let Some(script) = script_value(source, context, start, end)
                    {
                        ranges.push(ScriptRange {
                            start,
                            end,
                            script,
                            flow: false,
                        });
                    }
                }
            }
        }
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_inline_script_ranges(child, source, ranges, jsx_names, context, scope);
        }
    }
}

fn collect_markdown_flow_scripts(
    html: &markdown::mdast::Html,
    source: &str,
    ranges: &mut Vec<ScriptRange>,
    scope: &mut RawTextScope,
) {
    let Some(position) = &html.position else {
        return;
    };
    let value = &html.value;
    let mut indentation = 0;
    let Some((_, mut remaining, mut content)) =
        next_flow_script(value.trim_start_matches([' ', '\t']), scope)
    else {
        return;
    };
    let mut line_ends = value
        .split_inclusive(['\r', '\n'])
        .zip(source[position.start.offset..position.end.offset].split_inclusive(['\r', '\n']))
        .scan(
            (0, position.start.offset),
            |(value_end, source_end), (line, raw)| {
                *value_end += line.len();
                *source_end += raw.len();
                Some((*value_end, *source_end))
            },
        )
        .peekable();
    let mut source_offset = |offset| {
        while let Some(&(value_end, source_end)) = line_ends.peek() {
            if offset < value_end || offset == value_end && offset == value.len() {
                return source_end.checked_sub(value_end - offset);
            }
            line_ends.next();
        }
        None
    };
    while let Some(tail) = raw_text_end_tail(content, "script") {
        let start = value.len() - remaining.len() - indentation;
        let end = value.len() - tail.len();
        let (Some(source_start), Some(source_end)) = (source_offset(start), source_offset(end))
        else {
            break;
        };
        ranges.push(ScriptRange {
            start: source_start,
            end: source_end,
            script: value[start..end].to_string(),
            flow: true,
        });
        let Some((offset, next, next_content)) = next_flow_script(tail, scope) else {
            break;
        };
        indentation = tail[..offset]
            .rsplit_once(['\r', '\n'])
            .map(|(_, line)| line)
            .filter(|line| line.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
            .map_or(0, str::len);
        remaining = next;
        content = next_content;
    }
}

fn next_flow_script<'a>(
    tail: &'a str,
    scope: &mut RawTextScope,
) -> Option<(usize, &'a str, &'a str)> {
    let mut remaining = tail;
    loop {
        match *scope {
            RawTextScope::Comment => {
                remaining = comment_end_tail(remaining)?;
                *scope = RawTextScope::None;
                continue;
            }
            RawTextScope::Element(name) => {
                if name == "plaintext" {
                    return None;
                }
                remaining = raw_text_end_tail(remaining, name)?;
                *scope = RawTextScope::None;
                continue;
            }
            RawTextScope::None => {}
        }
        let offset = tail.len() - remaining.len();
        let less_than = remaining.find('<')?;
        let after = &remaining[less_than + 1..];
        let position = offset + less_than;
        if let Some(comment) = after.strip_prefix("!--") {
            match comment_end_tail(comment) {
                Some(after_comment) => remaining = after_comment,
                None => {
                    *scope = RawTextScope::Comment;
                    return None;
                }
            }
            continue;
        }
        let (closing, tag) = match after.strip_prefix('/') {
            Some(tag) => (true, tag),
            None => (false, after),
        };
        if !tag.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            remaining = after;
            continue;
        }
        let name_end = tag
            .find(|c| html_whitespace(c) || matches!(c, '/' | '>'))
            .unwrap_or(tag.len());
        let (name, attributes) = tag.split_at(name_end);
        let end = scan_tag_end(attributes, |_| {});
        let after_tag = attributes.get(end.offset + 1..).unwrap_or_default();
        if !closing && name.eq_ignore_ascii_case("script") {
            if let Some(content) = opening_tag_end(&tail[position..], "script") {
                return Some((position, &tail[position..], content));
            }
            remaining = after;
            continue;
        }
        if !closing && let Some(raw) = raw_text_name(name) {
            if raw == "plaintext" {
                *scope = RawTextScope::Element("plaintext");
                return None;
            }
            match raw_text_end_tail(after_tag, raw) {
                Some(after_raw) => remaining = after_raw,
                None => {
                    *scope = RawTextScope::Element(raw);
                    return None;
                }
            }
            continue;
        }
        remaining = after_tag;
    }
}

fn raw_text_end_tail<'a>(content: &'a str, name: &str) -> Option<&'a str> {
    let attributes = content
        .match_indices("</")
        .find_map(|(start, _)| tag_attributes(&content[start + 2..], name))?;
    attributes.get(scan_tag_end(attributes, |_| {}).offset + 1..)
}

fn line_start(source: &str, offset: usize) -> usize {
    source[..offset]
        .rfind(['\r', '\n'])
        .map_or(0, |end| end + 1)
}

fn project_list_openers(projection: &mut String, context: &ScriptContext<'_>, before_line: usize) {
    let Some((opener, parent)) = &context.list_opener else {
        return;
    };
    project_list_openers(projection, parent, opener.line_start.min(before_line));
    if opener.line_start < before_line {
        projection.push_str(opener.prefix);
        if opener.has_content {
            projection.push_str("<!-- -->");
        }
        projection.push('\n');
    }
}

fn script_value(
    source: &str,
    context: &ScriptContext<'_>,
    start: usize,
    end: usize,
) -> Option<String> {
    let raw = &source[start..end];
    if !raw.contains('\n') && !raw.contains('\r') {
        return Some(raw.to_string());
    }
    let content_start = context.inline_start.unwrap_or(start);
    let content_line = line_start(source, content_start);
    let mut projection = String::new();
    project_list_openers(&mut projection, context, content_line);
    projection.push_str(&source[content_line..content_start]);
    let script_start = projection.len();
    projection.push_str(raw);
    let mut parse = markdown::ParseOptions::default();
    parse.constructs.code_indented = false;
    let ast = markdown::to_mdast(&projection, &parse).ok()?;
    if let Some(html) = html_node_at(&ast, &projection, script_start)
        && html.position.as_ref()?.end.offset == projection.len()
    {
        return Some(html.value.trim_start_matches([' ', '\t']).to_string());
    }

    const RAW_TAG: &str = "<span data-script=\"";
    projection.truncate(script_start);
    projection.push_str("x ");
    projection.push_str(RAW_TAG);
    projection.push_str(&raw.replace('&', "&amp;").replace('"', "&quot;"));
    projection.push_str("\">");
    let ast = markdown::to_mdast(&projection, &parse).ok()?;
    let html = html_node_at(&ast, &projection, script_start + 2)?;
    if html.position.as_ref()?.end.offset != projection.len() {
        return None;
    }
    Some(
        html.value
            .strip_prefix(RAW_TAG)?
            .strip_suffix("\">")?
            .replace("&quot;", "\"")
            .replace("&amp;", "&"),
    )
}

fn html_node_at<'a>(
    node: &'a markdown::mdast::Node,
    source: &str,
    offset: usize,
) -> Option<&'a markdown::mdast::Html> {
    if let markdown::mdast::Node::Html(html) = node
        && html.position.as_ref().is_some_and(|position| {
            offset < position.end.offset
                && source
                    .get(position.start.offset..offset)
                    .is_some_and(|prefix| prefix.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
        })
    {
        return Some(html);
    }
    node.children()?
        .iter()
        .find_map(|child| html_node_at(child, source, offset))
}

enum PhrasingEvent<'a> {
    Tag(usize, usize, &'a str),
    CommentOpen,
    CommentClose,
}

fn collect_phrasing_events<'a>(
    node: &'a markdown::mdast::Node,
    source: &str,
    events: &mut Vec<(usize, PhrasingEvent<'a>)>,
) {
    match node {
        markdown::mdast::Node::Html(html) => {
            if let Some(position) = &html.position {
                events.push((
                    position.start.offset,
                    PhrasingEvent::Tag(position.start.offset, position.end.offset, &html.value),
                ));
            }
        }
        markdown::mdast::Node::Text(text) => {
            if let Some(position) = &text.position
                && let Some(text_source) = source.get(position.start.offset..position.end.offset)
            {
                collect_text_comment_events(text_source, position.start.offset, events);
            }
        }
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_phrasing_events(child, source, events);
        }
    }
}

fn collect_text_comment_events(
    text: &str,
    base: usize,
    events: &mut Vec<(usize, PhrasingEvent<'_>)>,
) {
    let mut index = 0;
    while let Some(found) = text[index..].find(['<', '-']) {
        let at = index + found;
        let rest = &text[at..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            if !is_escaped(text, at) && comment_end_tail(comment).is_none() {
                events.push((base + at, PhrasingEvent::CommentOpen));
            }
            index = at + 4;
        } else if rest.starts_with("-->") {
            events.push((base + at, PhrasingEvent::CommentClose));
            index = at + 3;
        } else if rest.starts_with("--!>") {
            events.push((base + at, PhrasingEvent::CommentClose));
            index = at + 4;
        } else {
            index = at + 1;
        }
    }
}

fn is_escaped(text: &str, offset: usize) -> bool {
    text[..offset]
        .bytes()
        .rev()
        .take_while(|&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn is_script_tag(tag: &str, closing: bool) -> bool {
    let prefix = if closing { "</" } else { "<" };
    let Some(rest) = tag.strip_prefix(prefix) else {
        return false;
    };
    let Some(name) = rest.get(.."script".len()) else {
        return false;
    };
    name.eq_ignore_ascii_case("script")
        && match rest["script".len()..].chars().next() {
            Some(c) => html_whitespace(c) || c == '>' || (!closing && c == '/'),
            None => true,
        }
}

fn html_tag_name(tag: &str) -> Option<(&str, bool)> {
    let (closing, rest) = match tag.strip_prefix("</") {
        Some(rest) => (true, rest),
        None => (false, tag.strip_prefix('<')?),
    };
    let name_end = rest
        .find(|c| html_whitespace(c) || matches!(c, '/' | '>'))
        .unwrap_or(rest.len());
    Some((&rest[..name_end], closing))
}

fn html_raw_text_name(tag: &str) -> Option<&'static str> {
    let (name, closing) = html_tag_name(tag)?;
    if closing {
        return None;
    }
    raw_text_name(name)
}

fn is_html_end_tag(tag: &str, name: &str) -> bool {
    html_tag_name(tag)
        .is_some_and(|(tag_name, closing)| closing && tag_name.eq_ignore_ascii_case(name))
}

fn restore_inline_scripts(
    html: String,
    scripts: ScriptPlaceholders<'_>,
) -> Result<String, Vec<bool>> {
    let Some(first) = scripts.first() else {
        return Ok(html);
    };
    let (prefix, _) = first
        .marker
        .rsplit_once(':')
        .expect("script marker includes an id");
    let prefix = prefix.trim_start_matches('.');
    let markers: HashMap<&str, usize> = scripts
        .iter()
        .enumerate()
        .map(|(index, placeholder)| (placeholder.marker.as_str(), index))
        .collect();
    let mut counts = vec![0; scripts.len()];
    let mut matches = Vec::with_capacity(scripts.len());
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find(prefix) {
        let start = cursor + offset;
        let end = html[start..]
            .find("-->")
            .map_or(html.len(), |end| start + end + 3);
        let matched = markers
            .get(&html[start..end])
            .map(|&index| (start, index))
            .or_else(|| {
                (start > cursor && html.as_bytes()[start - 1] == b'.')
                    .then(|| markers.get(&html[start - 1..end]))
                    .flatten()
                    .map(|&index| (start - 1, index))
            });
        if let Some((marker_start, index)) = matched {
            let placeholder = &scripts[index];
            let span_start = marker_start
                .checked_sub(placeholder.before.len())
                .filter(|&span| {
                    html.as_bytes().get(span..marker_start) == Some(placeholder.before.as_bytes())
                });
            let span_end = end.checked_add(placeholder.after.len()).filter(|&span| {
                html.as_bytes().get(end..span) == Some(placeholder.after.as_bytes())
            });
            if let (Some(span_start), Some(span_end)) = (span_start, span_end) {
                counts[index] += 1;
                matches.push((span_start, span_end, index));
            }
        }
        cursor = end;
    }
    if counts.iter().any(|&count| count != 1) {
        return Err(counts.into_iter().map(|count| count != 1).collect());
    }
    let mut restored = String::with_capacity(html.len());
    let mut cursor = 0;
    for (start, end, index) in matches {
        restored.push_str(&html[cursor..start]);
        restored.push_str(scripts[index].script);
        cursor = end;
    }
    restored.push_str(&html[cursor..]);
    Ok(restored)
}

fn render_out_of_band(
    block: &Block,
    id: &str,
    registry: &RendererRegistry,
    language: Language,
) -> String {
    match registry.render(id, &block.content) {
        RenderOutcome::Svg(markup) => {
            format!("<figure class=\"mt-render mt-{id}\">{markup}</figure>")
        }
        RenderOutcome::Failed(diag) => {
            let title = match &block.kind {
                BlockKind::Diagram(kind) => {
                    crate::i18n::diagram_failure_label(language, &kind.to_string())
                }
                _ => format!("{} rendering failed", diag.source),
            };
            diagnostic_html(&title, &diag.message, &block.content)
        }
    }
}

fn render_mdx_body(doc: &Document, registry: &RendererRegistry, language: Language) -> String {
    if let Some(diag) = doc.diagnostics().iter().find(|diag| diag.source == "mdx") {
        return diagnostic_html(
            &format!("{} rendering failed", diag.source),
            &diag.message,
            doc.source(),
        );
    }

    let mut out = String::new();
    let mut markdown_run = String::new();

    for block in doc.blocks() {
        if let Some(id) = block.renderer_id() {
            flush_markdown(&mut out, &mut markdown_run, doc);
            out.push_str(&render_out_of_band(block, id, registry, language));
            continue;
        }
        match block.kind {
            BlockKind::Mdx(kind) => {
                flush_markdown(&mut out, &mut markdown_run, doc);
                if kind == mt_core::document::block::MdxKind::JsxElement
                    && mt_core::document::outline::jsx_tag_name(&block.content).as_deref()
                        == Some("script")
                    && !block.content.ends_with("/>")
                {
                    out.push_str(&block.content);
                } else {
                    out.push_str(&mdx_placeholder(kind, &block.content));
                    if kind == mt_core::document::block::MdxKind::JsxElement {
                        let parse = mt_core::document::doc::parse_options(DocType::Mdx);
                        let (scripts, _) = collect_script_ranges(&block.content, &parse);
                        for range in scripts {
                            out.push_str(&range.script);
                        }
                    }
                }
            }
            _ => {
                markdown_run.push_str(&doc.source()[block.range.clone()]);
                markdown_run.push_str("\n\n");
            }
        }
    }
    flush_markdown(&mut out, &mut markdown_run, doc);

    out
}

fn mdx_placeholder(kind: mt_core::document::block::MdxKind, source: &str) -> String {
    let label = match kind {
        mt_core::document::block::MdxKind::JsxElement => {
            mt_core::document::outline::jsx_tag_name(source)
                .map(|n| format!("&lt;{n} /&gt;"))
                .unwrap_or_else(|| "JSX".into())
        }
        other => other.label().to_string(),
    };
    format!(
        "<div class=\"mt-mdx\"><span class=\"mt-mdx-label\">{label}</span><pre>{}</pre></div>",
        escape(source)
    )
}

fn diagnostic_html(title: &str, message: &str, original: &str) -> String {
    let original = if original.trim().is_empty() {
        String::new()
    } else {
        format!("<pre>{}</pre>", escape(original))
    };
    format!(
        "<div class=\"mt-error\"><div class=\"mt-error-title\">{}</div><div class=\"mt-error-msg\">{}</div>{original}</div>",
        escape(title),
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
th { background: var(--table-header-bg, rgba(127,127,127,.12)); color: var(--text-secondary, inherit); }
mark { background: var(--highlight-bg, rgba(240,200,60,.35)); color: inherit; }
blockquote { border-left: 3px solid var(--blockquote-border, rgba(127,127,127,.4));
             color: var(--text-secondary, inherit); margin-left: 0; padding-left: 1.1em; }
img, svg { max-width: 100%; }
.mt-render { margin: 1.4em 0; text-align: center; }
.mt-error { border: 1px solid #d9534f; border-left-width: 4px; border-radius: 8px;
            padding: 10px 14px; margin: 1.2em 0; }
.mt-error-title { font-weight: 600; color: var(--danger-color); }
.mt-error-msg   { white-space: pre-wrap; margin: .4em 0; font-size: .92em; }
.mt-mdx { border: 1px dashed var(--border-color, rgba(127,127,127,.6)); border-radius: 8px;
          padding: 8px 12px; margin: 1.2em 0; }
.mt-mdx-label { font-family: ui-monospace, monospace; font-size: .85em;
                color: var(--text-muted, inherit); }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::{Handle, NodeData, RcDom};
    use nsh_core::{Color, prelude::*};

    const DOCUMENT_POLICY: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:">"#;

    fn registry() -> RendererRegistry {
        RendererRegistry::with_defaults()
    }

    /// The DOM the page goes through (html5ever, as a browser reads it).
    fn parse(html: &str) -> RcDom {
        html5ever::parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .expect("parse the page")
    }

    /// Every element named `name` (HTML names are lowercase) under `handle`,
    /// in document order.
    fn elements(handle: &Handle, name: &str) -> Vec<Handle> {
        fn walk(handle: &Handle, name: &str, found: &mut Vec<Handle>) {
            if let NodeData::Element { name: tag, .. } = &handle.data
                && &*tag.local == name
            {
                found.push(handle.clone());
            }
            for child in handle.children.borrow().iter() {
                walk(child, name, found);
            }
        }
        let mut found = Vec::new();
        walk(handle, name, &mut found);
        found
    }

    /// The one element named `name` (the structure under test has exactly
    /// one).
    fn only(dom: &RcDom, name: &str) -> Handle {
        let found = elements(&dom.document, name);
        assert_eq!(found.len(), 1, "exactly one <{name}>");
        found[0].clone()
    }

    fn attribute(handle: &Handle, name: &str) -> Option<String> {
        let NodeData::Element { attrs, .. } = &handle.data else {
            return None;
        };
        attrs
            .borrow()
            .iter()
            .find(|attr| &*attr.name.local == name)
            .map(|attr| attr.value.to_string())
    }

    /// The first element child of `handle` (a text node or a comment is not
    /// a child the head's order is about).
    fn first_element_child(handle: &Handle) -> Handle {
        handle
            .children
            .borrow()
            .iter()
            .find(|child| matches!(child.data, NodeData::Element { .. }))
            .cloned()
            .expect("an element child")
    }

    fn is_csp_meta(handle: &Handle) -> bool {
        matches!(&handle.data, NodeData::Element { name, .. } if &*name.local == "meta")
            && attribute(handle, "http-equiv")
                .is_some_and(|value| value.eq_ignore_ascii_case("content-security-policy"))
    }

    /// Every element's name, in document order.
    fn element_order(handle: &Handle) -> Vec<String> {
        fn walk(handle: &Handle, order: &mut Vec<String>) {
            if let NodeData::Element { name, .. } = &handle.data {
                order.push(name.local.to_string());
            }
            for child in handle.children.borrow().iter() {
                walk(child, order);
            }
        }
        let mut order = Vec::new();
        walk(handle, &mut order);
        order
    }

    /// The one `<svg>…</svg>` in `html`, parsed as XML (the last close is
    /// the root's; an SVG may nest its own).
    fn parse_svg(html: &str) -> roxmltree::Document<'_> {
        let start = html.find("<svg").expect("an svg element");
        let end = html.rfind("</svg>").expect("the svg's end") + "</svg>".len();
        roxmltree::Document::parse(&html[start..end]).expect("well-formed svg")
    }

    /// The `<body>` content only. The stylesheet names every CSS class, so a
    /// whole-document `contains` check would match markup that never rendered.
    fn body(html: &str) -> &str {
        let start = html.find("<body>").expect("body") + "<body>".len();
        let end = html.rfind("</body>").expect("/body");
        &html[start..end]
    }

    fn css_color(html: &str, name: &str) -> Color {
        let prefix = format!("{name}: ");
        let value = html
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
            .expect("CSS color variable");
        css_hex(value)
    }

    fn css_rule_color(html: &str, selector: &str, property: &str) -> Color {
        let selector = format!("{selector} {{");
        let rule = html
            .split_once(&selector)
            .expect("CSS rule")
            .1
            .split_once('}')
            .expect("CSS rule end")
            .0;
        let property = format!("{property}: ");
        let value = rule
            .split(';')
            .find_map(|declaration| declaration.trim().strip_prefix(&property))
            .expect("CSS rule color");
        value
            .strip_prefix("var(")
            .and_then(|variable| variable.strip_suffix(')'))
            .map(|variable| css_color(html, variable))
            .unwrap_or_else(|| css_hex(value))
    }

    fn css_hex(value: &str) -> Color {
        let value = value
            .trim()
            .trim_end_matches(';')
            .strip_prefix('#')
            .expect("hex color");
        let [red, green, blue] = std::array::from_fn(|index| {
            u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).expect("hex channel")
        });
        Color::rgba(red, green, blue, 0xff)
    }

    fn contrast_ratio(foreground: Color, background: Color) -> f32 {
        let background = Hsla::from(background);
        background
            .blend(Hsla::from(foreground))
            .contrast_ratio(background)
    }

    fn md(src: &str) -> Document {
        Document::with_type(DocType::Markdown, src.to_string())
    }

    fn inline_placeholder(marker: String, script: &str) -> ScriptPlaceholder<'_> {
        ScriptPlaceholder {
            marker,
            script,
            before: "",
            after: "",
        }
    }

    #[test]
    fn renders_ordinary_markdown() {
        let html = page(&md("# Title\n\nSome *text*.\n"), &registry(), false);
        assert!(html.contains("<h1>Title</h1>"), "got {html}");
        assert!(html.contains("<em>text</em>"));
    }

    #[test]
    fn math_renders_to_svg() {
        let html = page(&md("$$\n\\frac{a}{b}\n$$\n"), &registry(), false);
        let svg = parse_svg(body(&html));
        assert_eq!(svg.root_element().tag_name().name(), "svg");
    }

    #[test]
    fn failed_render_shows_a_diagnostic_and_keeps_the_source() {
        let src = "```d2\nthis is the original source\n```\n";
        let html = page(&md(src), &RendererRegistry::new(), false);
        assert!(body(&html).contains("mt-error"));
        assert!(html.contains("this is the original source"));
        assert!(html.contains("no renderer registered"));
    }

    #[test]
    fn escapes_html_in_diagnostics() {
        let out = diagnostic_html("test", "<script>alert(1)</script>", "<img onerror=x>");
        assert!(!out.contains("<script>"), "got {out}");
        assert!(out.contains("&lt;script&gt;"));
        assert!(out.contains("&lt;img onerror=x&gt;"));
    }

    #[test]
    fn cjk_and_tables_survive() {
        let src = "# 标题\n\n| 列一 | 列二 |\n|---|---|\n| 值 | 值 |\n";
        let html = page(&md(src), &registry(), false);
        assert!(html.contains("标题"));
        assert!(html.contains("<table>"), "got {html}");
        assert!(
            html.contains(
                "th { background: var(--table-header-bg, rgba(127,127,127,.12)); color: var(--text-secondary, inherit); }"
            ),
            "Web table headers must consume the same secondary foreground token as Native"
        );
    }

    #[test]
    fn the_color_scheme_and_palette_follow_the_app_mode() {
        let doc = md("# x\n");
        for (dark, scheme, background, foreground) in [
            (false, "light", "#ffffff", "#24292f"),
            (true, "dark", "#0d1117", "#e6edf3"),
        ] {
            let html = page(&doc, &registry(), dark);
            assert!(html.contains(&format!("color-scheme: {scheme};")));
            assert!(!html.contains("color-scheme: light dark"));
            assert!(html.contains(&format!("--bg-color: {background};")));
            assert!(html.contains(&format!("--text-color: {foreground};")));
            assert!(html.contains("var(--bg-color"));
            assert!(html.contains("var(--text-color"));
            assert!(html.contains("default-src 'none'"));
        }
    }

    #[test]
    fn failed_diagram_titles_use_theme_danger_with_aa_contrast() {
        let doc = md("```mermaid\nnot a diagram\n```\n");
        for dark in [false, true] {
            let html = page(&doc, &registry(), dark);
            assert!(body(&html).contains("class=\"mt-error-title\""));
            let danger = css_rule_color(&html, ".mt-error-title", "color");
            let background = css_color(&html, "--bg-color");
            let contrast = contrast_ratio(danger, background);
            assert!(contrast >= 4.5, "theme danger contrast was {contrast:.2}:1");
            let token = if dark {
                crate::ui::tokens::DARK.danger
            } else {
                crate::ui::tokens::LIGHT.danger
            };
            assert_eq!(danger, token);
        }
    }

    #[test]
    fn block_order_is_preserved_around_diagrams() {
        let src = "First.\n\n$$\nx\n$$\n\nLast.\n";
        let html = page(&md(src), &registry(), false);
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

    fn page(doc: &Document, registry: &RendererRegistry, dark: bool) -> String {
        frame_document(&build_html(doc, registry, dark))
    }

    fn frame_document(host: &str) -> String {
        let (_, iframe) = host
            .split_once("<iframe sandbox=\"allow-scripts\" srcdoc=\"")
            .unwrap();
        let (encoded, tail) = iframe.split_once("\"></iframe>").unwrap();
        assert_eq!(tail, format!("{HOST_RELAY}</body></html>"));
        encoded.replace("&quot;", "\"").replace("&amp;", "&")
    }

    #[test]
    fn the_host_carries_exactly_one_frame_and_the_frame_relay() {
        let author = r#"<script>const text = '"</iframe>&lt;&quot;';</script>"#;
        let doc = Document::with_type(DocType::Html, author.into());
        let host = build_html(&doc, &registry(), false);
        let dom = parse(&host);
        let frame = only(&dom, "iframe");
        assert_eq!(
            attribute(&frame, "sandbox").as_deref(),
            Some("allow-scripts")
        );
        assert_eq!(
            elements(&dom.document, "script").len(),
            1,
            "the host page runs only the relay"
        );
        assert!(
            is_csp_meta(&first_element_child(&only(&dom, "head"))),
            "the host policy is the head's first element"
        );
        let order = element_order(&dom.document);
        let frame_at = order
            .iter()
            .position(|name| name == "iframe")
            .expect("the frame");
        let script_at = order
            .iter()
            .position(|name| name == "script")
            .expect("the relay");
        assert!(
            frame_at < script_at,
            "the host runs no script before the frame"
        );
        // The author's script stays inside the frame: the host's bytes carry
        // its exact escaped spelling.
        assert!(
            host.contains(r#"<script>const text = '&quot;</iframe>&amp;lt;&amp;quot;';</script>"#),
            "{host}"
        );
        assert!(
            host.ends_with(&format!("{HOST_RELAY}</body></html>")),
            "the relay is the host's last script"
        );
        let document = frame_document(&host);
        assert_eq!(
            document,
            format!("<!doctype html>{DOCUMENT_POLICY}{FRAME_PRELUDE}{author}")
        );
    }

    #[test]
    fn generated_pages_install_csp_before_scripts_or_components_on_first_build() {
        let diagram = r#"<svg><text id="node"></text></svg>"#;
        let script =
            r#"<script>document.getElementById("node").textContent = "Diagram label";</script>"#;
        let registry = registry();
        let markdown = page(&md(&format!("{diagram}\n\n{script}")), &registry, false);
        let mdx = page(
            &Document::with_type(DocType::Mdx, "# Title\n\n<Chart data={rows} />\n".into()),
            &registry,
            false,
        );
        for (page, author) in [(&markdown, script), (&mdx, "<div class=\"mt-mdx\">")] {
            let dom = parse(page);
            assert!(
                is_csp_meta(&first_element_child(&only(&dom, "head"))),
                "the policy is the head's first element: {page}"
            );
            let policies = elements(&dom.document, "meta")
                .iter()
                .filter(|meta| is_csp_meta(meta))
                .count();
            assert_eq!(policies, 1, "one policy per page: {page}");
            assert!(page.contains(author), "{page}");
        }
        assert!(markdown.find(diagram).unwrap() < markdown.find(script).unwrap());
        assert!(mdx.contains("&lt;Chart /&gt;"));
        assert!(mdx.contains("<h1>Title</h1>"));
    }

    #[test]
    fn html_preserves_doctype_variants_and_inserts_csp_into_attributed_heads() {
        let doctypes = [
            "<!DOCTYPE html>",
            "<!DOCTYPE  html>",
            "<!DoCtYpE\tHTML>",
            "<!doctype\nhtml>",
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\">",
            "<!doctype html PUBLIC  \"-//W3C//DTD HTML 4.01//EN\"  \"http://www.w3.org/TR/html4/strict.dtd\">",
            "<!DOCTYPE html SYSTEM \"about:legacy-compat\">",
        ];
        let heads = [
            "<head id=\"author\" data-mode='one' style='color: red'>",
            "<HEAD lang=en data-label=\"the head\">",
            r#"<head id=a onclick="x()">"#,
            "<head onload='run()'>",
            "<head onclick=x() data-path=a/b hidden>",
            r#"<head onclick='x("a>b")' data-label=">">"#,
        ];
        let author = "<title>Author</title><script>run()</script>";

        for doctype in doctypes {
            for head in heads {
                let html = r#"<html lang=en class='page' style="color: red" onclick="run()">"#;
                let source = format!("{doctype}{html}{head}{author}</head></html>");
                let doc = Document::with_type(DocType::Html, source);
                let expected = format!(
                    "{doctype}{html}{head}{DOCUMENT_POLICY}{FRAME_PRELUDE}{author}</head></html>"
                );
                assert_eq!(
                    page(&doc, &registry(), false),
                    expected,
                    "doctype: {doctype}, head: {head}"
                );
            }
        }
    }

    #[test]
    fn markdown_inline_script_keeps_its_raw_text() {
        let source = r#"A paragraph with <script>document.body.dataset.x="1"</script> and MTMARKDOWNRAWSCRIP&#84;0END."#;
        let html = page(&md(source), &registry(), false);
        let script = r#"<script>document.body.dataset.x="1"</script>"#;

        assert!(
            html.contains(&format!(
                "<p>A paragraph with {script} and MTMARKDOWNRAWSCRIPT0END.</p>"
            )),
            "inline script was changed during Markdown compilation: {html}"
        );
        assert_eq!(html.matches(script).count(), 1, "{html}");
    }

    #[test]
    fn markdown_inline_scripts_drop_blockquote_and_list_container_prefixes() {
        let expected = "<script>\nvoid 0;\n</script>";
        let mut failures = Vec::new();
        for source in [
            "> Note <script>\n> void 0;\n> </script>",
            "- Note <script>\n  void 0;\n  </script>",
        ] {
            let html = page(&md(source), &registry(), false);
            let count = html.matches(expected).count();
            if count != 1 {
                failures.push((source, count));
            }
        }
        assert!(
            failures.is_empty(),
            "script restoration failed: {failures:?}"
        );
    }

    #[test]
    fn markdown_inline_script_in_a_later_nested_list_paragraph_keeps_its_text() {
        let source = "- outer\n  - inner\n\n    Note <script>\n    void 0;\n    </script>";
        let html = page(&md(source), &registry(), false);
        assert!(body(&html).contains("<p>Note <script>\nvoid 0;\n</script></p>"));
    }

    #[test]
    fn mdx_inline_script_in_a_blockquote_keeps_its_text() {
        let source = "> Note <script>\n> void 0;</script>";
        let doc = Document::with_type(DocType::Mdx, source.into());
        let html = page(&doc, &registry(), false);
        assert!(body(&html).contains("<p>Note <script>\nvoid 0;</script></p>"));
    }

    #[test]
    fn markdown_heading_and_table_scripts_keep_their_raw_text() {
        let script = r#"<script>document.body.dataset.x="A & B"</script>"#;
        let registry = registry();
        let heading = page(&md(&format!("# Title {script}")), &registry, false);
        assert!(body(&heading).contains(&format!("<h1>Title {script}</h1>")));
        let table = page(
            &md(&format!("| Column |\n| --- |\n| {script} |")),
            &registry,
            false,
        );
        assert!(body(&table).contains(&format!("<td>{script}</td>")));
    }

    #[test]
    fn markdown_multiline_script_in_setext_heading_keeps_its_text() {
        let expected = "<h1>Title <script>\nvoid 0;</script></h1>";
        let registry = registry();
        for source in [
            "Title <script>\nvoid 0;</script>\n=====",
            "- Title <script>\n  void 0;</script>\n  =====",
        ] {
            let html = page(&md(source), &registry, false);
            assert!(body(&html).contains(expected), "{source}\n{}", body(&html));
        }
    }

    #[test]
    fn mdx_nested_flow_scripts_survive_rendering() {
        let script = "<script>\nvoid 0;\n</script>";
        let registry = registry();
        let failures: Vec<_> = [
            ("> <script>\n> void 0;\n> </script>", script),
            ("- <script>\n  void 0;\n  </script>", script),
            (
                "<Widget>\n  <script>\n  void 0;\n  </script>\n</Widget>",
                "<script>\n  void 0;\n  </script>",
            ),
            (
                "<Widget>\n    <script>\n    void 0;\n    </script>\n</Widget>",
                "<script>\n    void 0;\n    </script>",
            ),
        ]
        .into_iter()
        .filter(|&(source, expected)| {
            let doc = Document::with_type(DocType::Mdx, source.into());
            let html = page(&doc, &registry, false);
            body(&html).matches(expected).count() != 1
        })
        .collect();
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn mdx_script_elements_keep_their_raw_text() {
        let script = r#"<script>document.body.dataset.x="A & B"</script>"#;
        let registry = registry();
        let doc = Document::with_type(DocType::Mdx, script.into());
        assert!(body(&page(&doc, &registry, false)).contains(script));
        let doc = Document::with_type(DocType::Mdx, format!("# Title {script}"));
        assert!(body(&page(&doc, &registry, false)).contains(&format!("<h1>Title {script}</h1>")));
        let flow = "<script>\ndocument.body.dataset.x=\"A & B\"\n</script>";
        let doc = Document::with_type(DocType::Mdx, flow.into());
        assert_eq!(body(&page(&doc, &registry, false)), flow);
    }

    #[test]
    fn installs_the_frame_prelude_after_csp_and_before_authored_scripts() {
        let author = "<script>author()</script>";
        let registry = registry();
        let dns_off = r#"<meta http-equiv="x-dns-prefetch-control" content="off">"#;
        for kind in [DocType::Html, DocType::Markdown, DocType::Mdx] {
            let doc = Document::with_type(kind, author.into());
            let document = page(&doc, &registry, false);
            let (_, after_policy) = document.split_once(DOCUMENT_POLICY).unwrap();
            let (before_author, _) = after_policy.split_once(author).unwrap();
            assert!(!before_author.contains("window.navigation"), "{kind:?}");
            assert!(
                before_author
                    .strip_prefix(dns_off)
                    .is_some_and(|tail| tail.starts_with("<script>")),
                "{kind:?}"
            );
            for name in [
                "RTCPeerConnection",
                "webkitRTCPeerConnection",
                "RTCDataChannel",
                "RTCIceTransport",
                "RTCIceCandidate",
                "RTCRtpSender",
                "RTCRtpReceiver",
                "RTCRtpTransceiver",
                "RTCSctpTransport",
                "RTCDtlsTransport",
                "RTCSessionDescription",
            ] {
                assert!(
                    before_author.contains(&format!("\"{name}\"")),
                    "{kind:?}: {name}"
                );
            }
            assert!(before_author.contains(r#"addEventListener("click", followLink, true)"#));
            assert!(before_author.contains(r#"addEventListener("auxclick", followLink, true)"#));
            assert!(
                before_author.contains(
                    r#"addEventListener("submit", event => event.preventDefault(), true)"#
                )
            );
        }
    }

    #[test]
    fn the_frame_prelude_cancels_navigation_and_forwards_only_openable_links() {
        let document = page(&md("[link](https://example.com/)\n"), &registry(), false);
        assert!(
            document.contains(r#"if (!link) return null;"#),
            "{document}"
        );
        assert!(document.contains(r#"event.preventDefault();"#));
        assert!(document.contains(
            r#"return scheme && ["http", "https", "mailto"].includes(scheme[1].toLowerCase()) ? href : null;"#
        ));
        assert!(document.contains(r#"if (url !== null) parent.postMessage(url, "*");"#));
        assert!(
            document
                .contains(r#"addEventListener("submit", event => event.preventDefault(), true)"#)
        );
    }

    #[test]
    fn the_host_relays_only_strings_sent_by_the_document_frame() {
        let host = build_html(&md("# x\n"), &registry(), false);
        let dom = parse(&host);
        assert_eq!(
            elements(&dom.document, "script").len(),
            1,
            "the host page must run only the relay"
        );

        let (_, relay) = host.split_once("\"></iframe>").unwrap();
        assert!(relay.starts_with(HOST_RELAY), "{relay}");
        assert!(relay.contains(r#"const frame = document.querySelector("iframe");"#));
        let guard = r#"if (event.source !== frame.contentWindow || typeof event.data !== "string") return;"#;
        let guard_at = relay.find(guard).expect("relay checks source and type");
        let forward_at = relay
            .find(r#"window.nsh.postMessage(event.data)"#)
            .expect("relay forwards to the bridge");
        assert!(guard_at < forward_at, "{relay}");
    }

    #[test]
    fn the_frame_sandbox_admits_scripts_and_no_escape_token() {
        let host = build_html(&md("# x\n"), &registry(), false);
        let dom = parse(&host);
        let frame = only(&dom, "iframe");
        let sandbox = attribute(&frame, "sandbox").expect("the sandbox attribute");
        let tokens: Vec<&str> = sandbox.split_whitespace().collect();
        assert_eq!(tokens, ["allow-scripts"]);
        for forbidden in [
            "allow-popups",
            "allow-top-navigation",
            "allow-forms",
            "allow-same-origin",
            "allow-modals",
            "allow-downloads",
        ] {
            assert!(!tokens.contains(&forbidden), "{forbidden} in {sandbox}");
        }
    }

    #[test]
    fn html_trivia_accepts_legal_comments_before_doctype_html_and_head() {
        let registry = registry();
        let head = r#"<head id="author" data-mode='one'>"#;
        let author = "<title>Author</title></head>";
        let policy = format!("{DOCUMENT_POLICY}{FRAME_PRELUDE}");

        let mut failures = Vec::new();
        for comment in ["<!---->", "<!-- -- -->"] {
            for (placement, source, expected) in [
                (
                    "before doctype",
                    format!("{comment}<!doctype html><html>{head}{author}"),
                    format!("{comment}<!doctype html><html>{head}{policy}{author}"),
                ),
                (
                    "before html",
                    format!("<!doctype html>{comment}<html>{head}{author}"),
                    format!("<!doctype html>{comment}<html>{head}{policy}{author}"),
                ),
                (
                    "before head",
                    format!("<!doctype html><html>{comment}{head}{author}"),
                    format!("<!doctype html><html>{comment}{head}{policy}{author}"),
                ),
            ] {
                let doc = Document::with_type(DocType::Html, source);
                let actual = page(&doc, &registry, false);
                if actual != expected {
                    failures.push(format!("{placement} with {comment:?}: got {actual:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "head was not preserved: {failures:#?}");
    }

    #[test]
    fn mdx_inline_script_after_code_span_is_not_merged_into_a_delimiter() {
        let script = r#"<script>document.body.dataset.x="ran"</script>"#;
        let doc = Document::with_type(DocType::Mdx, format!("`code`{script}"));
        let rendered_page = page(&doc, &registry(), false);
        let rendered = body(&rendered_page);

        assert_eq!(
            rendered.trim_end(),
            format!("<p><code>code</code>{script}</p>")
        );
    }

    #[test]
    fn mdx_inline_script_preserves_adjacent_emphasis_and_appears_once() {
        let script = "<script>void 0</script>";
        let doc = Document::with_type(DocType::Mdx, format!("_label_{script}"));
        let rendered_page = page(&doc, &registry(), false);
        assert_eq!(
            body(&rendered_page).trim_end(),
            format!("<p><em>label</em>{script}</p>")
        );
    }

    #[test]
    fn mdx_inline_script_does_not_activate_other_inline_jsx() {
        let script = "<script>void 0</script>";
        let doc = Document::with_type(
            DocType::Mdx,
            format!("before <link rel=\"stylesheet\" /> <Widget>label</Widget> {script}"),
        );
        let rendered_page = page(&doc, &registry(), false);
        assert_eq!(
            body(&rendered_page).trim_end(),
            format!("<p>before  label {script}</p>")
        );
    }

    #[test]
    fn markdown_html_blocks_keep_all_scripts_in_order() {
        let consecutive = "<script>void 0;</script><script>void 1;</script>";
        let three = "<script>void 0;</script><script>void 1;</script><SCRIPT>void 2;</SCRIPT>";
        let separated = "<script>void 0;</script><em>kept</em><script>void 1;</script>";
        for (source, expected) in [
            (consecutive, consecutive),
            (three, three),
            (separated, separated),
            (
                "> <script>\r\n> void 0;\r\n> </script><em>kept</em><script>void 1;</script>",
                "<script>\r\nvoid 0;\r\n</script><em>kept</em><script>void 1;</script>",
            ),
        ] {
            let html = page(&md(source), &registry(), false);
            assert!(body(&html).contains(expected), "{:?}", body(&html));
        }
    }

    #[test]
    fn markdown_indented_script_in_type_one_block_keeps_later_blocks() {
        let source = "<script>void 0;</script >\n    <script>void 1;</script>\n\n# After";
        let html = page(&md(source), &registry(), false);
        let rendered = body(&html);
        assert!(rendered.contains("<h1>After</h1>"), "{rendered}");
        for script in ["<script>void 0;</script >", "<script>void 1;</script>"] {
            assert!(rendered.contains(script), "{rendered}");
        }
    }

    #[test]
    fn markdown_script_in_lazy_blockquote_continuation_keeps_later_blocks() {
        let source =
            "> Note <script>\n> const value = \"&quot;\";\nvoid value;\n> </script>\n\n# After";
        let html = page(&md(source), &registry(), false);
        let rendered = body(&html);
        assert!(rendered.contains("<h1>After</h1>"), "{rendered}");
        assert!(
            rendered.contains("<script>\nconst value = \"&quot;\";\nvoid value;\n</script>"),
            "{rendered}"
        );
    }

    #[test]
    fn markdown_flow_scripts_accept_html_end_tag_syntax() {
        let space = "<script>void 0;</script >";
        let tab = "<script>void 1;</SCRIPT\t>";
        let attributes = "<script>void 2;</script ignored=\">\">";
        for scripts in [vec![space], vec![tab], vec![space, tab, attributes]] {
            let source = scripts.concat();
            let (ranges, _) = collect_script_ranges(&source, &markdown::ParseOptions::default());
            let extracted: Vec<_> = ranges.iter().map(|range| range.script.as_str()).collect();
            assert_eq!(extracted, scripts);
        }
    }

    #[test]
    fn document_link_templates_keep_head_and_body_siblings() {
        let source = concat!(
            "<!doctype html><html><head><link rel=stylesheet data-label='a>b'>",
            "<title>Preview</title></head><body><main>Body</main>",
            "<link rel=stylesheet data-label=\"c>d\" /></body></html>",
        );
        let html = page(
            &Document::with_type(DocType::Html, source.into()),
            &registry(),
            false,
        );
        assert!(html.ends_with(concat!(
            "<template data-mt-link rel=stylesheet data-label='a>b'></template>",
            "<title>Preview</title></head><body><main>Body</main>",
            "<template data-mt-link rel=stylesheet data-label=\"c>d\" ></template></body></html>",
        )), "{html}");
        assert_eq!(
            neutralize_document_links("<link rel=stylesheet data-label='a>b"),
            "<template data-mt-link rel=stylesheet data-label='a>b'></template>"
        );
    }

    #[test]
    fn document_links_are_made_inert_in_every_context() {
        let source = concat!(
            "<link rel=preconnect><LINK rel=dns-prefetch><link/rel=preload>",
            "<svg><title><link rel=preload></title></svg>",
            "<linker><link-data><link\u{e9}>",
        );
        let expected = concat!(
            "<template data-mt-link rel=preconnect></template>",
            "<template data-mt-link rel=dns-prefetch></template>",
            "<template data-mt-link/rel=preload></template>",
            "<svg><title><template data-mt-link rel=preload></template></title></svg>",
            "<linker><link-data><link\u{e9}>",
        );
        let html = page(
            &Document::with_type(DocType::Html, source.into()),
            &registry(),
            false,
        );
        assert!(html.ends_with(expected), "{html}");
    }

    #[test]
    fn document_link_with_many_attributes_is_made_inert() {
        use std::fmt::Write as _;

        let mut attributes = String::new();
        for index in 0..50_000 {
            write!(attributes, " a{index}").unwrap();
        }
        let source = format!("<link rel=preload{attributes}>");
        let started = std::time::Instant::now();
        let html = page(
            &Document::with_type(DocType::Html, source),
            &registry(),
            false,
        );
        let elapsed = started.elapsed();
        assert!(html.ends_with(&format!(
            "<template data-mt-link rel=preload{attributes}></template>"
        )));
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "50,000 attributes took {elapsed:?}"
        );
    }

    #[test]
    fn makes_document_links_inert_without_editing_other_markup() {
        let cases = [
            (
                "<LiNk ReL=PrEcOnNeCt href='about:blank'>",
                "<template data-mt-link ReL=PrEcOnNeCt href='about:blank'></template>",
            ),
            (
                "<link href='about:blank' rel='alternate\tDNS-prefetch'>",
                "<template data-mt-link href='about:blank' rel='alternate\tDNS-prefetch'></template>",
            ),
            (
                "<link disabled rel=prefetch href='about:blank'>",
                "<template data-mt-link disabled rel=prefetch href='about:blank'></template>",
            ),
            (
                "<link rel='preload' href='about:blank' />",
                "<template data-mt-link rel='preload' href='about:blank' ></template>",
            ),
            (
                "<link href='about:blank' rel=modulepreload>",
                "<template data-mt-link href='about:blank' rel=modulepreload></template>",
            ),
            (
                "<link rel=prerender>",
                "<template data-mt-link rel=prerender></template>",
            ),
            (
                "<link rel=preload data-label='<link rel=preload>'><link rel=prefetch>",
                concat!(
                    "<template data-mt-link rel=preload data-label='&lt;link rel=preload>'></template>",
                    "<template data-mt-link rel=prefetch></template>",
                ),
            ),
            (
                "<link rel=stylesheet href='theme.css'>",
                "<template data-mt-link rel=stylesheet href='theme.css'></template>",
            ),
            ("<!-- <link rel=preload> -->", "<!-- <link rel=preload> -->"),
        ];
        let registry = registry();
        let mut failures = Vec::new();
        for kind in [DocType::Html, DocType::Markdown] {
            for (markup, expected) in cases {
                let doc = Document::with_type(kind, format!("<div>{markup}end</div>"));
                let rendered = page(&doc, &registry, false);
                if !rendered.contains(&format!("<div>{expected}end</div>")) {
                    failures.push((kind, markup));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn preserves_links_in_raw_text_and_resumes_after_the_end_tag() {
        let source = concat!(
            "<script>const tag = /<link>/;</SCRIPT ignored=\">\">",
            "<style>p::before{content:'<link rel=preload>'}</style>",
            "<textarea><link rel=preload></textarea>",
            "<title><link></title><xmp><link></xmp><iframe><link></iframe>",
            "<noembed><link></noembed><noframes><link></noframes>",
            "<noscript><link></noscript><!-- <link><script> -->",
            "<script>const tag = /<link>/;</script ><link rel=preload>",
            "<plaintext><link></plaintext><link>",
        );
        assert_eq!(
            neutralize_document_links(source),
            source.replace(
                "</script ><link rel=preload>",
                "</script ><template data-mt-link rel=preload></template>"
            )
        );
    }

    #[test]
    fn deeply_nested_markup_neutralizes_links_within_a_bounded_time() {
        let opening = "<div>".repeat(50_000);
        let closing = "</div>".repeat(50_000);
        let source = format!("{opening}<link rel=preload>{closing}");
        let started = std::time::Instant::now();
        let filtered = neutralize_document_links(&source);
        let elapsed = started.elapsed();

        assert_eq!(
            filtered,
            format!("{opening}<template data-mt-link rel=preload></template>{closing}")
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "filter took {elapsed:?} for {} UTF-16 units",
            source.encode_utf16().count()
        );
    }

    #[test]
    fn misnested_tables_near_the_host_limit_neutralize_links_within_a_bounded_time() {
        let trailing = "</table><link rel=preload>";
        let target_len = MAX_HOST_UTF16_UNITS - 1;
        let mut source = String::with_capacity(target_len);
        source.push_str("<table>");
        while source.len() + "<p></p>".len() + trailing.len() <= target_len {
            source.push_str("<p></p>");
        }
        let fill = target_len - source.len() - trailing.len();
        source.push_str(&"x".repeat(fill));
        source.push_str(trailing);
        assert_eq!(source.len(), target_len);

        let started = std::time::Instant::now();
        let filtered = neutralize_document_links(&source);
        let elapsed = started.elapsed();

        assert_eq!(
            filtered.strip_suffix("<template data-mt-link rel=preload></template>"),
            source.strip_suffix("<link rel=preload>")
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "filter took {elapsed:?} for {} UTF-16 units",
            source.encode_utf16().count()
        );
    }

    #[test]
    fn mdx_script_marker_cannot_be_forged_with_character_references() {
        let script = r#"<script>document.body.dataset.x="ran"</script>"#;
        let source = format!("`code`{script} and MTMARKDOWNRAWSCRIP&#84;0END.");
        let doc = Document::with_type(DocType::Mdx, source);
        let html = page(&doc, &registry(), false);
        let rendered = body(&html);

        assert_eq!(rendered.matches(script).count(), 1, "{rendered}");
        assert!(
            rendered.contains("and MTMARKDOWNRAWSCRIPT0END."),
            "{rendered}"
        );
    }

    #[test]
    fn missing_inline_script_marker_leaves_the_other_script_and_content() {
        let source =
            "before <script>void 0;</script> between <script>void 1;</script> after\n\n# After";
        let mut options = markdown::Options::gfm();
        options.compile.allow_dangerous_html = true;
        let mut calls = 0;
        let rendered = render_markdown_scripts(source, &mut options, |source, options| {
            let mut html = markdown::to_html_with_options(source, options).unwrap();
            calls += 1;
            if calls == 1 {
                let start = html.rfind("<!--mt-script:").unwrap();
                html.replace_range(start..start + 1, "&lt;");
            }
            Ok(html)
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(
            rendered,
            "<p>before <script>void 0;</script> between &lt;script>void 1;&lt;/script> after</p>\n<h1>After</h1>"
        );
    }

    #[test]
    fn inline_script_restoration_rejects_missing_or_duplicate_markers() {
        let marker = "<!--mt-script:0123456789abcdef0123456789abcdef:0-->".to_string();
        let script = "<script>author()</script>";

        assert_eq!(
            restore_inline_scripts(
                String::new(),
                vec![inline_placeholder(marker.clone(), script)]
            ),
            Err(vec![true])
        );
        assert_eq!(
            restore_inline_scripts(
                format!("{marker}{marker}"),
                vec![inline_placeholder(marker, script)]
            ),
            Err(vec![true])
        );
    }

    #[test]
    fn inline_script_restoration_leaves_an_unclaimed_marker_inert() {
        let prefix = "<!--mt-script:0123456789abcdef0123456789abcdef";
        assert_eq!(
            restore_inline_scripts(
                format!("{prefix}:0-->{prefix}:1-->"),
                vec![inline_placeholder(
                    format!("{prefix}:0-->"),
                    "<script>void 0</script>"
                )],
            ),
            Ok(format!("<script>void 0</script>{prefix}:1-->"))
        );
    }

    #[test]
    fn inline_script_restoration_near_the_host_limit_is_bounded() {
        let prefix = "<!--mt-script:0123456789abcdef0123456789abcdef";
        let mut html = String::new();
        let mut scripts = Vec::new();
        let script = "<script>void 0</script>";
        for id in 0.. {
            let marker = format!("{prefix}:{id}-->");
            if html.len() + marker.len() >= MAX_HOST_UTF16_UNITS {
                break;
            }
            html.push_str(&marker);
            scripts.push(inline_placeholder(marker, script));
        }
        let expected = script.repeat(scripts.len());
        let started = std::time::Instant::now();
        let restored = restore_inline_scripts(html, scripts).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "restoration took {elapsed:?}"
        );
        assert!(restored == expected, "restoration changed the scripts");
    }

    #[test]
    fn inline_script_placeholder_generation_on_one_long_line_is_bounded() {
        let script = "<script>void 0</script>";
        let source = script.repeat(100_000);
        let ranges: Vec<_> = source
            .match_indices(script)
            .map(|(start, script)| ScriptRange {
                start,
                end: start + script.len(),
                script: script.to_string(),
                flow: false,
            })
            .collect();
        let started = std::time::Instant::now();
        std::hint::black_box(protect_script_ranges(&source, &ranges, &[], true).unwrap());
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "placeholder generation took {elapsed:?}"
        );
    }

    #[test]
    fn multiline_scripts_in_a_list_with_a_long_first_line_are_bounded() {
        let script = "<script>\nvoid 0;\n</script>";
        let count = 10_000;
        let source = format!(
            "- {}\n\n{}",
            "a".repeat(300_000),
            "  Note <script>\n  void 0;\n  </script>\n\n".repeat(count)
        );
        let ast = markdown::to_mdast(&source, &markdown::ParseOptions::default()).unwrap();
        let mut ranges = Vec::new();
        let started = std::time::Instant::now();
        collect_inline_script_ranges(
            &ast,
            &source,
            &mut ranges,
            &mut Vec::new(),
            &ScriptContext::default(),
            &mut RawTextScope::None,
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "{elapsed:?}"
        );
        assert!(
            ranges.len() == count && ranges.iter().all(|range| range.script == script),
            "list scripts were lost or changed"
        );
    }

    #[test]
    fn rejects_host_pages_over_the_windows_string_load_limit() {
        let registry = registry();
        let build = |text| {
            let doc = Document::with_type(DocType::Html, text);
            build_html(&doc, &registry, false)
        };
        let prefix = "<p>&\"";
        let suffix = "\u{1f642}</p>";
        let overhead = build(format!("{prefix}{suffix}")).encode_utf16().count();
        let capacity = 1024 * 1024 - overhead;
        for size in [capacity - 1, capacity] {
            let host = build(format!("{prefix}{}{suffix}", "a".repeat(size)));
            assert_eq!(host.encode_utf16().count(), overhead + size);
            assert!(host.contains("srcdoc=\""));
        }
        let above = build(format!("{prefix}{}{suffix}", "a".repeat(capacity + 1)));
        assert!(above.contains("This preview is too large to display."));
        assert!(!above.contains("<iframe"));
    }

    #[test]
    fn checks_host_size_after_making_document_links_inert() {
        let registry = registry();
        let build = |source| {
            build_html(
                &Document::with_type(DocType::Html, source),
                &registry,
                false,
            )
        };
        let hint = r#"<link rel="preload" href="fixture.css" as="style">"#;
        let inert =
            r#"<template data-mt-link rel="preload" href="fixture.css" as="style"></template>"#;
        let overhead = build(format!("<p>{inert}</p>")).encode_utf16().count();
        let text = "a".repeat(MAX_HOST_UTF16_UNITS - overhead);
        let exact = build(format!("<p>{hint}{text}</p>"));
        assert!(exact.contains("srcdoc=\""));
        assert_eq!(exact.encode_utf16().count(), MAX_HOST_UTF16_UNITS);
        assert_eq!(exact, build(format!("<p>{inert}{text}</p>")));
        let above = build(format!("<p>{hint}{text}a</p>"));
        assert!(above.contains("This preview is too large to display."));
    }

    #[test]
    fn foreign_object_keeps_script_raw_text_and_neutralizes_real_links() {
        let script = "<script>const tag = /<link>/;</script>";
        let source = format!(
            "<svg><foreignObject>{script}</foreignObject></svg>\
             <svg><foreignObject><link rel=preload></foreignObject></svg>\
             <svg><foreignObject></foreignObject><script>const b = /<link>/;</script></svg>"
        );
        let filtered = neutralize_document_links(&source);
        assert!(
            filtered.contains(script),
            "foreignObject script changed: {filtered}"
        );
        assert!(
            filtered.contains(
                "<svg><foreignObject><template data-mt-link rel=preload></template></foreignObject></svg>"
            ),
            "foreignObject link survived: {filtered}"
        );
        assert!(
            filtered.contains(
                "<svg><foreignObject></foreignObject><script>const b = /<template data-mt-link></template>/;</script></svg>"
            ),
            "SVG content after foreignObject was not scanned: {filtered}"
        );

        let document = page(
            &Document::with_type(DocType::Html, source),
            &registry(),
            false,
        );
        assert!(
            document.contains(script),
            "page changed the script: {document}"
        );
        assert!(!document.contains("<link rel=preload>"), "{document}");
    }

    #[test]
    fn flow_placeholders_keep_a_type_one_blocks_other_html() {
        let source = concat!(
            "<script>void 0;</script >\n",
            "    <div>Keep me</div>\n",
            "<script>void 1;</script>\n",
            "\n# After\n",
        );
        let html = page(&md(source), &registry(), false);
        let rendered = body(&html);
        assert!(
            rendered.contains(
                "<script>void 0;</script >\n    <div>Keep me</div>\n<script>void 1;</script>"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("<h1>After</h1>"), "{rendered}");
        assert!(
            !rendered.contains("&lt;div&gt;Keep me&lt;/div&gt;"),
            "{rendered}"
        );
    }

    #[test]
    fn examples_inside_flow_raw_text_stay_examples() {
        let example = "<script>void 1;</script>";
        let mut failures = Vec::new();
        for name in [
            "textarea", "title", "xmp", "noscript", "style", "iframe", "noembed", "noframes",
        ] {
            let source = format!("<script>void 0;</script><{name}>{example}</{name}>");
            let html = page(&md(&source), &registry(), false);
            let rendered = body(&html);
            if !rendered.contains("<script>void 0;</script>") || rendered.contains(example) {
                failures.push((name, rendered.to_string()));
            }
        }
        let commented = concat!(
            "<script>void 0;</script >\n",
            "<!-- <script>void 1;</script> -->\n",
            "\n# After\n",
        );
        let html = page(&md(commented), &registry(), false);
        let rendered = body(&html);
        if rendered.contains("<script>void 1;</script>") {
            failures.push(("comment", rendered.to_string()));
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn examples_inside_inline_raw_text_stay_examples() {
        let source = "A <textarea><script>void 1;</script></textarea> B\n";
        let html = page(&md(source), &registry(), false);
        let rendered = body(&html);
        assert!(!rendered.contains("<script>void 1;</script>"), "{rendered}");
        assert!(rendered.contains("&lt;textarea>"), "{rendered}");
    }

    #[test]
    fn plaintext_keeps_the_rest_of_the_inline_context_inert() {
        let source = "A <plaintext><script>document.x=1</script></plaintext> \
                      <script>document.y=2</script> B";
        let html = page(&md(source), &registry(), false);
        let rendered = body(&html);
        assert!(!rendered.contains("<script>"), "{rendered}");
        assert!(
            rendered.contains("&lt;script>document.x=1&lt;/script>"),
            "{rendered}"
        );
        assert!(
            rendered.contains("&lt;script>document.y=2&lt;/script>"),
            "{rendered}"
        );
    }

    #[test]
    fn flow_scripts_stay_inert_while_raw_text_from_an_earlier_block_stays_open() {
        let script = "<script>void 0;</script>";
        let mut failures = Vec::new();
        for opener in [
            "<textarea>",
            "<title>",
            "<xmp>",
            "<noscript>",
            "<style>",
            "<iframe>",
            "<noembed>",
            "<noframes>",
            "<plaintext>",
        ] {
            let source = format!("A {opener}\n\n{script}\n");
            let html = page(&md(&source), &registry(), false);
            let rendered = body(&html);
            if rendered.contains(script) || !rendered.contains("&lt;script>void 0;&lt;/script>") {
                failures.push((opener, rendered.to_string()));
            }
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn flow_scripts_stay_inert_while_a_comment_from_an_earlier_block_stays_open() {
        let script = "<script>void 0;</script>";
        let source = format!("A <!--\n\n{script} -->\n");
        let html = page(&md(&source), &registry(), false);
        let rendered = body(&html);
        assert!(!rendered.contains(script), "{rendered}");
        assert!(
            rendered.contains("&lt;script>void 0;&lt;/script>"),
            "{rendered}"
        );
    }

    #[test]
    fn raw_text_scope_closed_in_a_later_block_keeps_scripts_running() {
        let script = "<script>void 1;</script>";
        for source in [
            format!("A <textarea>\n\nB </textarea> C\n\n{script}\n"),
            format!("<textarea>\n\nB </textarea> C\n\n{script}\n"),
            format!("A <!--\n\nB --> C\n\n{script}\n"),
        ] {
            let html = page(&md(&source), &registry(), false);
            assert!(body(&html).contains(script), "{source:?}: {html}");
        }
    }

    #[test]
    fn unclosed_raw_text_scope_scanning_stays_linear() {
        let source = "<textarea>\n\n<script>void 0;</script>\n\n".repeat(10_000);
        let ast = markdown::to_mdast(&source, &markdown::ParseOptions::default()).unwrap();
        let mut ranges = Vec::new();
        let started = std::time::Instant::now();
        collect_inline_script_ranges(
            &ast,
            &source,
            &mut ranges,
            &mut Vec::new(),
            &ScriptContext::default(),
            &mut RawTextScope::None,
        );
        let elapsed = started.elapsed();
        assert!(ranges.is_empty(), "{} scripts were collected", ranges.len());
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "{elapsed:?}"
        );
    }

    #[test]
    fn raw_text_scope_closed_in_a_flow_script_block_restores_later_scripts() {
        let suppressed = "<script>void 0;</script>";
        let restored = "<script>evil()</script>";
        let source = format!("A <textarea>\n\n{suppressed} </textarea> {restored}\n");
        let html = page(&md(&source), &registry(), false);
        let rendered = body(&html);
        assert!(!rendered.contains(suppressed), "{rendered}");
        assert!(
            rendered.contains("&lt;script>void 0;&lt;/script>"),
            "{rendered}"
        );
        assert!(rendered.contains(restored), "{rendered}");
    }

    #[test]
    fn raw_text_scope_closed_in_a_later_flow_block_restores_the_script_after_it() {
        let script = "<script>void 1;</script>";
        let source = format!("A <textarea>\n\n</textarea>\n{script}\n");
        let html = page(&md(&source), &registry(), false);
        let rendered = body(&html);
        assert!(rendered.contains(script), "{rendered}");
        assert!(!rendered.contains("&lt;script"), "{rendered}");
    }

    #[test]
    fn flow_block_without_a_script_start_restores_the_script_after_other_html() {
        let script = "<script>void 1;</script>";
        let source = format!("<div>\n<!-- x -->\n{script}\n");
        let html = page(&md(&source), &registry(), false);
        let rendered = body(&html);
        assert!(rendered.contains("<div>"), "{rendered}");
        assert!(rendered.contains(script), "{rendered}");
    }

    #[test]
    fn cross_block_raw_text_scope_scanning_stays_linear() {
        let script = "<script>void 0;</script>";
        let unit = format!("A <textarea>\n\nB </textarea> {script}\n\n");
        let count = 20_000;
        let source = unit.repeat(count);
        let ast = markdown::to_mdast(&source, &markdown::ParseOptions::default()).unwrap();
        let mut ranges = Vec::new();
        let started = std::time::Instant::now();
        collect_inline_script_ranges(
            &ast,
            &source,
            &mut ranges,
            &mut Vec::new(),
            &ScriptContext::default(),
            &mut RawTextScope::None,
        );
        let elapsed = started.elapsed();
        assert!(
            ranges.len() == count && ranges.iter().all(|range| range.script == script),
            "{} scripts were collected",
            ranges.len()
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "{elapsed:?}"
        );
    }

    #[test]
    fn nested_foreign_object_keeps_the_html_context_for_its_script() {
        let source = "<svg><foreignObject><foreignObject></foreignObject>\
                      <script>const tag = /<link>/;</script></foreignObject></svg>";
        assert_eq!(neutralize_document_links(source).as_ref(), source);
    }

    #[test]
    fn inline_script_restoration_does_not_slice_outside_char_boundaries() {
        let placeholder = ScriptPlaceholder {
            marker: "<!--mt-script:ABCD:0-->".to_string(),
            script: "<script>void 0</script>",
            before: "&lt;script>",
            after: "&lt;/script>",
        };
        assert_eq!(
            restore_inline_scripts(
                "\u{1f600}<script><!--mt-script:ABCD:0--></script>".to_string(),
                vec![placeholder]
            ),
            Err(vec![true])
        );
    }

    #[test]
    fn unrecoverable_flow_script_stays_tag_filtered_and_the_rest_renders() {
        let source = concat!(
            "<script>void 0;</script >\n",
            "    <div>Keep me</div>\n",
            "<script>void 1;</script>\n",
            "\n# After\n",
        );
        let mut options = markdown::Options::gfm();
        options.compile.allow_dangerous_html = true;
        let mut calls = 0;
        let rendered = render_markdown_scripts(source, &mut options, |source, options| {
            let mut html = markdown::to_html_with_options(source, options).unwrap();
            calls += 1;
            if calls == 1 {
                let start = html.rfind("<!--mt-script:").unwrap();
                html.replace_range(start..start + 1, "&lt;");
            }
            Ok(html)
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert!(rendered.contains("<script>void 0;</script >"), "{rendered}");
        assert!(!rendered.contains("<script>void 1;</script>"), "{rendered}");
        assert!(
            rendered.contains("&lt;script>void 1;&lt;/script>"),
            "{rendered}"
        );
        assert!(rendered.contains("<div>Keep me</div>"), "{rendered}");
        assert!(rendered.contains("<h1>After</h1>"), "{rendered}");
        assert!(!rendered.contains("mt-error"), "{rendered}");
    }

    #[test]
    fn invalid_mdx_script_shows_one_diagnostic_without_running_authored_scripts() {
        let source = "Before\n\n<script>String(1<2)</script>\n\nAfter\n\n<script>later()</script>";
        let doc = Document::with_type(DocType::Mdx, source.into());
        let html = page(&doc, &registry(), false);
        let rendered = body(&html);
        let diagnostic = &doc.diagnostics()[0];
        assert_eq!(
            rendered.trim(),
            diagnostic_html("mdx rendering failed", &diagnostic.message, source)
        );
    }

    #[test]
    fn markdown_filters_stray_raw_tags_before_later_blocks() {
        let script = "<ScRiPt>\nvoid 0;\n\nvoid 1;\n</sCrIpT>";
        let source = format!(
            "Intro <script>\n\n    </script>\n\n# After\n\n{script}<style>p{{display:none}}</style>"
        );
        let html = page(&md(&source), &registry(), false);
        let rendered = body(&html);
        assert!(rendered.contains("Intro &lt;script>"), "{rendered}");
        assert!(rendered.contains("<h1>After</h1>"), "{rendered}");
        assert!(
            rendered.contains(&format!("{script}&lt;style>")),
            "{rendered}"
        );
    }

    #[test]
    fn markdown_script_spanning_paragraphs_follows_commonmark() {
        let source =
            "Before <script>\nvoid 0;\n\nvoid 1;\n</script>\n\nAfter\n\n<script>later()</script>";
        let html = page(&md(source), &registry(), false);
        assert_eq!(
            body(&html).trim(),
            "<p>Before &lt;script>\nvoid 0;</p>\n<p>void 1;\n&lt;/script></p>\n<p>After</p>\n<script>later()</script>"
        );
    }

    #[test]
    fn html_inserts_csp_at_the_start_of_the_real_or_implied_head() {
        let registry = registry();
        let author = r#"<link rel="stylesheet" href="relative.css"><script>run()</script>"#;
        let inert = r#"<template data-mt-link rel="stylesheet" href="relative.css"></template><script>run()</script>"#;
        let policy = format!("{DOCUMENT_POLICY}{FRAME_PRELUDE}");
        for (prefix, expected_prefix) in [
            ("<!doctype html><html><head>", "<!doctype html><html><head>"),
            (
                "<!DOCTYPE HTML><HTML lang=en><HEAD data-x='>'>",
                "<!DOCTYPE HTML><HTML lang=en><HEAD data-x='>'>",
            ),
            (
                "<!-- <head> --><head>",
                "<!doctype html><!-- <head> --><head>",
            ),
            (
                "<html style=\"background:url(relative.png)\"><head>",
                "<!doctype html><html style=\"background:url(relative.png)\"><head>",
            ),
            ("<!DOCTYPE HTML><html>", "<!DOCTYPE HTML><html>"),
            ("\u{feff}<head>", "<!doctype html><head>"),
            ("", "<!doctype html>"),
        ] {
            let doc = Document::with_type(DocType::Html, format!("{prefix}{author}"));
            assert_eq!(
                page(&doc, &registry, true),
                format!("{expected_prefix}{policy}{inert}")
            );
        }
        for unsafe_prefix in [
            "<head data-x='",
            "<head",
            "<!--><script>early()</script>--><head>",
            "<!-- --!><script>early()</script>--><head>",
            "<!doctype x \"><script>early()</script>\"><head>",
            "<!DOCTYPE html PUBLIC \"><script>early()</script>\"><head>",
            "<!DOCTYPE html SYSTEM \"><script>early()</script>\"><head>",
        ] {
            let doc = Document::with_type(DocType::Html, format!("{unsafe_prefix}{author}"));
            assert_eq!(
                page(&doc, &registry, false),
                format!("<!doctype html>{policy}{unsafe_prefix}{inert}")
            );
        }
    }

    #[test]
    fn html_keeps_author_shell_styles_and_additional_policy() {
        let before = "<!doctype html><html><head>";
        let author = r#"<meta http-equiv="Content-Security-Policy" content="script-src 'none'"><style>p{color:red}</style></head><body><p>Hi</p></body></html>"#;
        let doc = Document::with_type(DocType::Html, format!("{before}{author}"));
        assert_eq!(
            page(&doc, &registry(), true),
            format!("{before}{DOCUMENT_POLICY}{FRAME_PRELUDE}{author}")
        );
    }

    #[test]
    fn diagram_and_math_use_the_registry_inside_the_frame() {
        let doc = md("```d2\na -> b\n```\n\n$$\nx^2\n$$\n");
        let page = page(&doc, &registry(), true);
        let body = body(&page);
        let diagram = body
            .find("<figure class=\"mt-render mt-d2\">")
            .expect("diagram");
        let math = body
            .find("<figure class=\"mt-render mt-math\">")
            .expect("math");
        assert!(diagram < math, "blocks out of order: {body}");
        for (name, region) in [("diagram", &body[diagram..math]), ("math", &body[math..])] {
            let svg = parse_svg(region);
            assert_eq!(svg.root_element().tag_name().name(), "svg", "{name}");
        }
    }
}
