//! The WebView compatibility path for browser-only document semantics.
//!
//! The application supplies a static host. Only its sandboxed document frame
//! contains author markup, with no file base or application bridge.

use std::borrow::Cow;

use html5ever::buffer_queue::BufferQueue;
use html5ever::tokenizer::{
    Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerResult,
};
use html5ever::tree_builder::{TreeBuilder, TreeBuilderOpts};
use markup5ever_rcdom::{Handle, RcDom};
use mt_core::rendering::{RenderOutcome, RendererRegistry};
use mt_core::{Block, BlockKind, DocType, Document};

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
    const cancelLink = event => {
        if (event.composedPath().some(node =>
            (node.localName === "a" || node.localName === "area") &&
            (node.hasAttribute("href") || node.hasAttribute("xlink:href")))) {
            event.preventDefault();
        }
    };
    addEventListener("click", cancelLink, true);
    addEventListener("auxclick", cancelLink, true);
    addEventListener("submit", event => event.preventDefault(), true);
})();</script>"#;

/// Build a base-free WebView host; only the opaque-origin frame runs scripts.
pub fn build_html(doc: &Document, registry: &RendererRegistry, dark: bool) -> String {
    let head_prefix = format!("{}{FRAME_PRELUDE}", csp_meta(""));
    let page = match doc.doc_type() {
        DocType::Html => build_html_raw(doc.source(), &head_prefix),
        kind => {
            let body = if kind == DocType::Mdx {
                render_mdx_body(doc, registry)
            } else {
                render_markdown_body(doc, registry)
            };
            format!(
                "<!doctype html><html><head>{head_prefix}<meta charset=\"utf-8\">\
                 <style>{vars}\n{STYLE}</style></head><body>{body}</body></html>",
                vars = css_variables(dark),
            )
        }
    };
    let page = strip_resource_hints(&page);
    let host = format!(
        "<!doctype html><html><head>{host_csp}<meta charset=\"utf-8\">\
         <style>iframe{{position:fixed;inset:0;width:100%;height:100%;border:0}}</style>\
         </head><body><iframe sandbox=\"allow-scripts\" srcdoc=\"{}\"></iframe></body></html>",
        escape_srcdoc(&page),
        host_csp = csp_meta(HOST_FRAME_SRC),
    );
    if host.encode_utf16().count() > MAX_HOST_UTF16_UNITS {
        format!(
            "<!doctype html><html><head>{}<meta charset=\"utf-8\"></head>\
             <body><p role=\"alert\">This preview is too large to display.</p></body></html>",
            csp_meta("")
        )
    } else {
        host
    }
}

fn strip_resource_hints(page: &str) -> Cow<'_, str> {
    let candidates = page.match_indices('<').filter_map(|(start, _)| {
        let tag = page.as_bytes().get(start + 1..start + 6)?;
        (tag[..4].eq_ignore_ascii_case(b"link")
            && (html_whitespace(char::from(tag[4])) || matches!(tag[4], b'/' | b'>')))
        .then_some(start)
    });
    let mut candidates = candidates.peekable();
    if candidates.peek().is_none() {
        return Cow::Borrowed(page);
    }

    let mut tokenizer = Tokenizer::new(ResourceHintTokens::default(), Default::default());
    let mut input = BufferQueue::default();
    let mut previous = 0;
    let mut copied = 0;
    let mut filtered = String::new();
    for (end, next_link) in candidates
        .map(|start| (start + 1, Some(start)))
        .chain(std::iter::once((page.len(), None)))
    {
        input.push_back(page[previous..end].into());
        while let TokenizerResult::Script(_) = tokenizer.feed(&mut input) {
            if let Some(start) = tokenizer.sink.hint_start.take() {
                filtered.push_str(&page[copied..start]);
                filtered.push_str("<!---->");
                copied = opening_tag_end(&page[start..], "link").map_or_else(
                    || end - queued_bytes(&mut input),
                    |rest| page.len() - rest.len(),
                );
            }
        }
        if tokenizer.sink.candidate.is_none() {
            tokenizer.sink.candidate = next_link;
        }
        previous = end;
    }
    if copied == 0 {
        Cow::Borrowed(page)
    } else {
        filtered.push_str(&page[copied..]);
        Cow::Owned(filtered)
    }
}

fn queued_bytes(input: &mut BufferQueue) -> usize {
    let mut tail = Vec::new();
    while let Some(chunk) = input.pop_front() {
        tail.push(chunk);
    }
    let bytes = tail.iter().map(|chunk| chunk.len()).sum();
    for chunk in tail {
        input.push_back(chunk);
    }
    bytes
}

struct ResourceHintTokens {
    tree: TreeBuilder<Handle, RcDom>,
    candidate: Option<usize>,
    hint_start: Option<usize>,
}

impl Default for ResourceHintTokens {
    fn default() -> Self {
        Self {
            tree: TreeBuilder::new(
                RcDom::default(),
                TreeBuilderOpts {
                    iframe_srcdoc: true,
                    ..TreeBuilderOpts::default()
                },
            ),
            candidate: None,
            hint_start: None,
        }
    }
}

impl TokenSink for ResourceHintTokens {
    type Handle = Option<Handle>;

    fn process_token(&mut self, token: Token, line: u64) -> TokenSinkResult<Self::Handle> {
        if !matches!(token, Token::ParseError(_)) {
            let candidate = self.candidate.take();
            if let Token::TagToken(tag) = &token
                && is_resource_hint(tag)
            {
                self.hint_start = candidate;
                // Pause after the tag to measure its original end in the input queue.
                return TokenSinkResult::Script(None);
            }
        }
        match self.tree.process_token(token, line) {
            TokenSinkResult::Continue => TokenSinkResult::Continue,
            TokenSinkResult::Plaintext => TokenSinkResult::Plaintext,
            TokenSinkResult::RawData(kind) => TokenSinkResult::RawData(kind),
            TokenSinkResult::Script(handle) => TokenSinkResult::Script(Some(handle)),
        }
    }

    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.tree
            .adjusted_current_node_present_but_not_in_html_namespace()
    }
}

fn is_resource_hint(tag: &Tag) -> bool {
    const HINTS: [&str; 6] = [
        "preconnect",
        "dns-prefetch",
        "prefetch",
        "preload",
        "modulepreload",
        "prerender",
    ];
    tag.kind == TagKind::StartTag
        && tag.name.as_ref() == "link"
        && tag.attrs.iter().any(|attr| {
            attr.name.local.as_ref() == "rel"
                && attr
                    .value
                    .split(html_whitespace)
                    .any(|token| HINTS.iter().any(|hint| token.eq_ignore_ascii_case(hint)))
        })
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

/// Recognize only ordinary opening tags and attributes, not HTML error recovery.
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

/// The original Light and Dark reading palettes, independent of UI toolkit.
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
    let mut options = markdown::Options {
        parse: mt_core::document::doc::parse_options(doc.doc_type()),
        compile: markdown::CompileOptions {
            // The source is a local file the user opened; raw HTML in it is
            // authored content, and blocking it would break ordinary
            // documents. The frame policy governs scripts and loads.
            allow_dangerous_html: true,
            allow_dangerous_protocol: false,
            ..markdown::CompileOptions::default()
        },
    };
    let rendered = preserve_inline_scripts(run, &options.parse).and_then(|(source, scripts)| {
        if !scripts.is_empty() {
            options.parse.constructs.html_text = true;
        }
        let html = markdown::to_html_with_options(source.as_ref(), &options)
            .map_err(|message| message.reason)?;
        restore_inline_scripts(html, scripts)
    });
    match rendered {
        Ok(html) => out.push_str(&html),
        Err(message) => out.push_str(&diagnostic_html("markdown", &message, run)),
    }
    run.clear();
}

type ScriptPlaceholders = Vec<(String, String)>;

fn preserve_inline_scripts<'a>(
    source: &'a str,
    parse_options: &markdown::ParseOptions,
) -> Result<(Cow<'a, str>, ScriptPlaceholders), String> {
    if !source
        .as_bytes()
        .windows(b"<script".len())
        .any(|tag| tag.eq_ignore_ascii_case(b"<script"))
    {
        return Ok((Cow::Borrowed(source), Vec::new()));
    }

    let Ok(ast) = markdown::to_mdast(source, parse_options) else {
        return Ok((Cow::Borrowed(source), Vec::new()));
    };
    let mut ranges = Vec::new();
    let mut jsx_names = Vec::new();
    collect_inline_script_ranges(&ast, source, &mut ranges, &mut jsx_names);
    ranges.sort_unstable_by_key(|(start, _)| *start);
    if ranges.is_empty() {
        return Ok((Cow::Borrowed(source), Vec::new()));
    }

    let marker_prefix = random_script_marker_prefix()?;
    let mut protected = String::with_capacity(source.len());
    let mut scripts = Vec::with_capacity(ranges.len());
    jsx_names.sort_unstable();
    let mut jsx_names = jsx_names.into_iter().peekable();
    let mut cursor = 0;
    for (index, (start, end)) in ranges.into_iter().enumerate() {
        if start < cursor {
            continue;
        }
        copy_mdx_text(&mut protected, source, cursor, start, &mut jsx_names);
        let line_prefix = source[..start]
            .rsplit_once('\n')
            .map_or(&source[..start], |(_, line)| line);
        // A punctuation guard keeps a line-leading comment in MDX's inline grammar.
        let guard = if parse_options.constructs.mdx_jsx_text
            && line_prefix
                .trim_start_matches(|c| html_whitespace(c) || c == '>')
                .is_empty()
        {
            "."
        } else {
            ""
        };
        let marker = format!("{guard}{marker_prefix}:{index}-->");
        protected.push_str(&marker);
        scripts.push((marker, source[start..end].to_string()));
        cursor = end;
    }
    copy_mdx_text(&mut protected, source, cursor, source.len(), &mut jsx_names);
    Ok((Cow::Owned(protected), scripts))
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

fn collect_inline_script_ranges(
    node: &markdown::mdast::Node,
    source: &str,
    ranges: &mut Vec<(usize, usize)>,
    jsx_names: &mut Vec<usize>,
) {
    use markdown::mdast::Node;
    if let Node::MdxJsxTextElement(jsx) = node
        && let Some(position) = &jsx.position
        && let Some(raw) = source.get(position.start.offset..position.end.offset)
    {
        let start = position.start.offset;
        if jsx.name.as_deref() == Some("script") && !raw.ends_with("/>") {
            ranges.push((start, position.end.offset));
            return;
        }
        jsx_names.push(start + 1);
        if !raw.ends_with("/>")
            && let Some(close) = raw.rfind("</")
        {
            jsx_names.push(start + close + 2);
        }
    }

    if matches!(
        node,
        Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_)
    ) {
        let mut tags = Vec::new();
        collect_html_tags(node, &mut tags);
        tags.sort_unstable_by_key(|(start, _, _)| *start);

        let mut opening = None;
        for (start, end, tag) in tags {
            if is_script_tag(tag, false) {
                opening.get_or_insert(start);
            } else if is_script_tag(tag, true)
                && let Some(start) = opening.take()
            {
                ranges.push((start, end));
            }
        }
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_inline_script_ranges(child, source, ranges, jsx_names);
        }
    }
}

fn collect_html_tags<'a>(node: &'a markdown::mdast::Node, tags: &mut Vec<(usize, usize, &'a str)>) {
    if let markdown::mdast::Node::Html(html) = node
        && let Some(position) = &html.position
    {
        tags.push((position.start.offset, position.end.offset, &html.value));
    }

    if let Some(children) = node.children() {
        for child in children {
            collect_html_tags(child, tags);
        }
    }
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

fn restore_inline_scripts(mut html: String, scripts: ScriptPlaceholders) -> Result<String, String> {
    for (marker, script) in &scripts {
        let mut matches = html.match_indices(marker);
        let Some((start, _)) = matches.next() else {
            return Err("an inline script placeholder was lost during Markdown rendering".into());
        };
        if matches.next().is_some() {
            return Err(
                "an inline script placeholder appeared more than once during Markdown rendering"
                    .into(),
            );
        }
        html.replace_range(start..start + marker.len(), script);
    }
    if let Some((prefix, _)) = scripts
        .first()
        .and_then(|(marker, _)| marker.rsplit_once(':'))
        && html.contains(prefix.trim_start_matches('.'))
    {
        return Err("an inline script placeholder remained after Markdown rendering".into());
    }
    Ok(html)
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
/// Raw script elements reach the frame; other JSX uses the shared placeholders.
fn render_mdx_body(doc: &Document, registry: &RendererRegistry) -> String {
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
                if kind == mt_core::document::block::MdxKind::JsxElement
                    && mt_core::document::outline::jsx_tag_name(&block.content).as_deref()
                        == Some("script")
                    && !block.content.ends_with("/>")
                {
                    out.push_str(&block.content);
                } else {
                    out.push_str(&mdx_placeholder(kind, &block.content));
                }
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

/// The document stylesheet.
///
/// Colors consume the light/dark variables above; translucent fallbacks keep
/// individual rules legible if document-authored styles override a variable.
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
.mt-error-title { font-weight: 600; color: #d9534f; }
.mt-error-msg   { white-space: pre-wrap; margin: .4em 0; font-size: .92em; }
.mt-mdx { border: 1px dashed var(--border-color, rgba(127,127,127,.6)); border-radius: 8px;
          padding: 8px 12px; margin: 1.2em 0; }
.mt-mdx-label { font-family: ui-monospace, monospace; font-size: .85em;
                color: var(--text-muted, inherit); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT_POLICY: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:">"#;
    const HOST_POLICY: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; frame-src 'self'">"#;

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
        let html = page(&md("# Title\n\nSome *text*.\n"), &registry(), false);
        assert!(html.contains("<h1>Title</h1>"), "got {html}");
        assert!(html.contains("<em>text</em>"));
    }

    #[test]
    fn math_renders_to_svg() {
        let html = page(&md("$$\n\\frac{a}{b}\n$$\n"), &registry(), false);
        assert!(body(&html).contains("<svg"), "got {}", body(&html));
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
        let (encoded, tail) = iframe.split_once('"').unwrap();
        assert_eq!(tail, "></iframe></body></html>");
        encoded.replace("&quot;", "\"").replace("&amp;", "&")
    }

    #[test]
    fn host_is_static_and_author_markup_cannot_escape_srcdoc() {
        let author = r#"<script>const text = '"</iframe>&lt;&quot;';</script>"#;
        let doc = Document::with_type(DocType::Html, author.into());
        let host = build_html(&doc, &registry(), false);
        assert!(host.starts_with(&format!("<!doctype html><html><head>{HOST_POLICY}")));
        assert_eq!(host.matches("<iframe ").count(), 1);
        let (before_srcdoc, encoded) = host.split_once(" srcdoc=\"").unwrap();
        assert!(!before_srcdoc.contains("<script"));
        assert!(
            encoded
                .contains(r#"<script>const text = '&quot;</iframe>&amp;lt;&amp;quot;';</script>"#)
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
        let prefix = format!("<!doctype html><html><head>{DOCUMENT_POLICY}");
        for (page, author) in [(&markdown, script), (&mdx, "<div class=\"mt-mdx\">")] {
            assert!(page.starts_with(&prefix), "{page}");
            assert!(page[prefix.len()..].contains(author), "{page}");
            assert_eq!(page.matches("Content-Security-Policy").count(), 1);
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
            assert!(before_author.contains(r#"addEventListener("click", cancelLink, true)"#));
            assert!(before_author.contains(r#"addEventListener("auxclick", cancelLink, true)"#));
            assert!(
                before_author.contains(
                    r#"addEventListener("submit", event => event.preventDefault(), true)"#
                )
            );
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
    fn strips_static_resource_hints_without_editing_other_markup() {
        let cases = [
            ("<LiNk ReL=PrEcOnNeCt href='about:blank'>", "<!---->"),
            (
                "<link href='about:blank' rel='alternate\tDNS-prefetch'>",
                "<!---->",
            ),
            ("<link disabled rel=prefetch href='about:blank'>", "<!---->"),
            ("<link rel='preload' href='about:blank' />", "<!---->"),
            ("<link href='about:blank' rel=modulepreload>", "<!---->"),
            ("<link rel=prerender>", "<!---->"),
            (
                "<link rel=stylesheet href='theme.css'>",
                "<link rel=stylesheet href='theme.css'>",
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
    fn inline_script_restoration_rejects_missing_or_duplicate_markers() {
        let marker = "MTMARKDOWNRAWSCRIPT0123456789abcdef0123456789abcdef0END".to_string();
        let script = "<script>author()</script>".to_string();

        assert!(
            restore_inline_scripts(String::new(), vec![(marker.clone(), script.clone())]).is_err()
        );
        assert!(
            restore_inline_scripts(format!("{marker}{marker}"), vec![(marker, script)]).is_err()
        );
    }

    #[test]
    fn inline_script_restoration_rejects_an_unclaimed_marker() {
        let prefix = "<!--mt-script:0123456789abcdef0123456789abcdef";
        assert!(
            restore_inline_scripts(
                format!("{prefix}:0-->{prefix}:1-->"),
                vec![(format!("{prefix}:0-->"), "<script>void 0</script>".into())],
            )
            .is_err()
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
    fn html_inserts_csp_at_the_start_of_the_real_or_implied_head() {
        let registry = registry();
        let author = r#"<link rel="stylesheet" href="relative.css"><script>run()</script>"#;
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
                format!("{expected_prefix}{policy}{author}")
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
            let source = format!("{unsafe_prefix}{author}");
            let doc = Document::with_type(DocType::Html, source.clone());
            assert_eq!(
                page(&doc, &registry, false),
                format!("<!doctype html>{policy}{source}")
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
        assert!(body[diagram..math].contains("<svg"), "{body}");
        assert!(body[math..].contains("<svg"), "{body}");
    }
}
