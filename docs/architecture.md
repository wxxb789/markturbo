# Architecture

The boundaries chosen for v0.1, and why. This is the note GOAL §19.7 asks for,
written after inspecting the upstream source rather than from assumptions.

## The one hard boundary

`mt-doc` — the document engine — has **no GPUI dependency**. That is the only
structural rule this codebase enforces strictly, because it is the one that
determines whether the model can later be reused by CLI tooling, MCP tools,
agent tools, headless rendering, or indexing.

Everything else is a module in `mt-app`, split by what it owns rather than by
layer. There is no `traits/`, no `services/`, no dependency-injection container.

```text
Filesystem
    │
    ▼
┌──────────────────────────────────────────┐
│ mt-doc                                   │
│   doctype    what kind of artifact       │
│   frontmatter YAML header extraction     │
│   doc        source + blocks + outline   │
│   block      renderer dispatch keys      │
│   diagnostic problems, never panics      │
│   skill      Agent Skills model          │
│   translate  what is translatable        │
└────────────────────┬─────────────────────┘
                     │
     ┌───────────────┼───────────────┬──────────────┐
     │               │               │              │
  mt-app::fs   mt-app::renderer  mt-app::web   mt-app::views
  load/save    diagram + math    WebView       GPUI
  conflicts    registry          + trust       ↓
                                          explorer · document
                                          skills · workspace
```

## Why these boundaries

### Document engine, not document widget

GPUI widgets are not the source of document semantics. `Document` owns the
source text and everything derived from it; views read it. The single mutation
path is `set_source`, so derived state cannot drift from the text.

Every derived structure carries byte spans back into the original source. That
is what makes "open and preview never modify the file" a property rather than a
hope, and what lets translation reassemble a document losslessly.

### One model, both renderers

The native path and the WebView path both consume the same `Document`. Neither
re-parses independently. `both_render_paths_agree_on_document_structure` asserts
this: if they diverged into parallel models, that test fails.

Native is the fast path. WebView is the compatibility path. Feature parity is
explicitly not a goal, and Markdown never round-trips through the WebView.

### Renderer registry, not special cases

`BlockRenderer` is looked up by `Block::renderer_id()`. Adding Graphviz means:
implement the trait, register it, add the fence language to
`DiagramKind::from_lang`. The Markdown parser, both renderers, and every view
are untouched. `a_new_renderer_needs_no_core_changes` asserts exactly this.

Availability is part of the trait, so a renderer needing an external binary
reports its absence as an actionable install hint rather than a mysterious
failure — and the document architecture stays uninfected by that dependency.

### Diagnostics, not errors

Nothing in the pipeline returns `Result` to the UI for content problems. A
malformed frontmatter, an invalid Mermaid diagram, unparseable MDX — all become
`Diagnostic` values attached to the document or block, with the original source
preserved. A broken file always opens and can always be repaired in the editor.

Third-party renderers run inside `catch_unwind`: a panic in a dependency becomes
a diagnostic, not a lost session.

## Key decisions

### gpui-component provides more than expected

Inspecting the upstream source first (GOAL §19) changed the plan substantially.
`gpui-component` already ships:

- `MarkdownExtensions` — a block parser/renderer registry with an MDX parse
  mode. The "renderer registry" GOAL §5 asks for at the *view* layer already
  exists upstream; this codebase supplies the block *backends* and lets
  `TextView` dispatch.
- `EditorState` / `Editor` — a rope-backed code editor with tree-sitter
  highlighting, undo/redo, and search already keybound. No editor was written.
- `TreeState` / `tree()` — virtualized tree. The explorer supplies items.
- `TabBar`, `h_resizable`, `TitleBar`, `ListItem`, theming.
- `gpui-wry` — a WebView element (Windows and macOS).

So `mt-app` is mostly wiring plus the four things upstream does not have:
document semantics, diagram backends, filesystem safety, and translation.

### Pure-Rust renderers where they exist

Verified against crates.io rather than assumed:

| Technology | Choice | Note |
|---|---|---|
| Mermaid | `mermaid-svg` 0.7 | Pure Rust, 23 diagram types |
| D2 | `d2-little` 0.7.1-1 | Pure Rust including layout. The version needs the exact prerelease tag; 0.7.2/0.7.3 are yanked |
| Math | `mathjax-svg-rs` 0.4 | MathJax on an embedded JS engine. Emits glyph outlines (`<path>`), not `<text>`, so it renders under resvg with no fonts installed — which the native path needs |
| PlantUML | `plantuml` CLI | `plantuml-little` depends on `graphviz-anywhere`, which *fails at build time* on Windows. Rejected |

Rejected for math: `latex2mathml` (abandoned 2020), `pulldown-latex` (MathML
only — resvg cannot draw MathML, so the native path would show nothing),
`mathjax_svg` 3.2 (embeds V8).

### MDX: structure natively, fidelity in the WebView

`markdown-rs` parses MDX constructs, so native mode identifies JSX elements,
ESM statements, and expressions, builds an outline from them, and never
corrupts the document. It does not evaluate them: there is no JS engine in the
document engine at all, which is also why opening executable content cannot run
it.

`markdown-rs` gates ESM behind an `mdx_esm_parse` callback. We supply one that
accepts every statement, because the structural boundary is a blank line, which
the parser already finds — and the WebView path, running a real compiler, stays
the authority on JS validity.

### Trust boundary

MDX can contain executable code, so the boundary had to be decided now even
though no runtime ships in v0.1. Content is served to the WebView as a `data:`
URL — an opaque origin with no filesystem access, no `file://` reach, and no
ambient credentials — under a CSP where `default-src 'none'` blocks **all**
network access at both trust levels. `Restricted`, the default for every
document, additionally blocks scripts. Trusting is explicit and per-document,
and never opens the network.

### Save safety

The filesystem is the source of truth, and agents write to it concurrently. A
save records the file's mtime and size at load; if disk no longer matches, the
save is refused and the user chooses reload or overwrite. Writes go to a sibling
temp file and are renamed, so a crash cannot truncate a document.

Line endings and BOM are detected on load and restored on save, so a file this
app opens and saves untouched is byte-identical.

### Background parsing

`markdown-rs` is superlinear in the *number of blocks*: on this repo's fixtures,
10x the input costs ~50x the time (~13s for 100K lines). This is upstream — the
same curve appears with the parser's own default constructs, and a single huge
paragraph with the same byte count scales linearly.

We cannot fix that, but we can make it not matter: `DocumentView` reparses on a
background executor after a 180ms debounce, and discards results that a newer
edit has superseded. The editor and the previous parse stay live meanwhile.
`a_huge_document_is_slow_enough_to_require_background_parsing` documents the
measurement that justifies the machinery — and will fail if it stops being true.

### Translation preserves structure, not by asking nicely

A prompt saying "don't translate code" is not a guarantee. Instead the document
engine splits a scope into translatable and verbatim segments that **tile the
range exactly** — concatenating them reproduces the source byte-for-byte — and
sends only prose. Fenced code, inline code, math, diagram source, MDX, link
targets, autolinks, frontmatter, block markup (`#`, `-`, `>`, `1.`), and table
pipes are all verbatim.

`markup_survives_a_provider_that_rewrites_everything` asserts the strong form:
even a provider returning unrelated text cannot damage document structure.

## What was deliberately not built

Per GOAL §18: no knowledge graph, backlinks, canvas, PKM system, proprietary
format, WYSIWYG, collaboration, sync, accounts, plugin marketplace, extension
runtime, terminal, debugger, LSP IDE, git client, vector DB, RAG, or AI chat
sidebar. Agents live in Codex, Claude Code, Cursor, and the terminal; this app
owns the workspace they consume.

Also not built, and why:

- **A dock/panel system.** `h_resizable` covers the required layout. `DockArea`
  is available upstream if freeform panels are ever wanted.
- **An HTTP client.** Translation shells out to `curl`, which ships on every
  supported platform. Adding a TLS stack to the graph for one optional endpoint
  is not worth the build cost.
- **A generic plugin platform.** The renderer registry is the extension point
  that was actually needed.

## Extension points

Ordered by how cheap they are:

1. **A diagram technology** — implement `BlockRenderer`, register it, add the
   fence language. Nothing else changes.
2. **A skill discovery convention** — one line in `skill::DISCOVERY_ROOTS`.
3. **A translation provider** — implement `TranslationService`, add a
   `Provider` variant.
4. **A document type** — a `DocType` variant plus its recognition rule.
5. **A split layout** (`Native | Web`, `Original | Translation`) — `ViewMode`
   and `PreviewKind` are already separate for this reason.

## Known limits

- PlantUML needs a local install. Everything else is self-contained.
- The WebView is Windows/macOS only, upstream. Linux gets an explanation in the
  Web pane rather than a broken one; native rendering is unaffected.
- MDX components render as placeholders in both trust levels. The trust boundary
  and CSP are in place for when a runtime ships.
- Live web preview pauses above 512KB; the editor and native preview stay live.
- Closing a tab with unsaved edits discards them without a prompt. The file on
  disk is never touched.
