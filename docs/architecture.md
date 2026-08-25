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
│   outline    headings + MDX structure    │
│   block      renderer dispatch keys      │
│   diagnostic problems, never panics      │
│   skill      Agent Skills model          │
│   harness    where skills live, per tool │
│   instruction agent instruction files    │
│   walk       which files are openable    │
│   search     find across documents       │
│   translate  what is translatable        │
└────────────────────┬─────────────────────┘
                     │
     ┌───────────────┼───────────────┬──────────────┐
     │               │               │              │
  mt-app::fs   mt-app::renderer  mt-app::web   mt-app::views
  load/save    diagram + math    WebView       GPUI
  encoding     registry          + trust       ↓
  conflicts                                    │
                                               │
     ┌─────────────────────────────────────────┤
     │                                         │
  workspace ────────────────────────────┐   panels
  the wiring layer                      │   explorer · harness
     ├── tabs         open set, preview │   search   · document
     ├── history      back / forward    │   settings_page
     └── web_surface  the OS child window
```

Each of those four under `workspace` was a cluster of fields inside it. They
moved out because nothing else touched them — which is also what made their
rules testable without a window. `tabs` is the clearest case: "closing a tab to
the left must not switch documents" was a real defect, and it is now an
assertion rather than something you find by clicking.

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
| Math | `ratex-parser` / `-layout` / `-types` / `-font` 0.1.14 | Pure Rust, no JS engine. The SVG is emitted here rather than by `ratex-svg` — see below |
| PlantUML | `plantuml` CLI | `plantuml-little` depends on `graphviz-anywhere`, which *fails at build time* on Windows. Rejected |

Rejected for math: `latex2mathml` (abandoned 2020), `pulldown-latex`, `Temml`
and `math-core` (all MathML only — resvg drops every non-SVG-namespaced element
at `usvg/parser/svgtree/parse.rs:139`, so the native path would show nothing),
`mathjax_svg` 3.2 (embeds V8), ReX (69/90 on a KaTeX corpus against RaTeX's
89/90, and unpublished).

### Math: RaTeX, minus its SVG emitter

`ratex-svg` is the obvious dependency and is deliberately absent. The only way
to get `<path>` rather than `<text>` out of it is its `standalone` feature, and
`standalone` reaches `ratex-unicode-font` through two independent edges — a
direct optional dependency, and a non-optional dependency of
`ratex-font-loader`. No feature combination avoids it.

That crate does three things this application cannot accept: it prints with
`eprintln!` past the `log` crate, so `RUST_LOG` cannot filter it; it hardcodes
five distro-specific font paths, which is how a font loader works on the
author's machine and nowhere else; and it reads a system CJK font it never
frees — measured 4.5MB to 52.0MB resident on the first CJK glyph, retained for
the process.

Emitting the SVG in `renderer.rs` instead is about 250 lines and removes all
three at the source. Measured after: 4.34MB at startup, 4.55MB after a thousand
formulas, zero lines on stderr, and no platform font path in the binary.

The emitter writes `<path>` for every glyph a KaTeX face covers — which is all
of mathematics — and falls through to `<text>` for the rest, meaning CJK inside
`\text` and emoji. Those resolve against the font database gpui already
populates from the system, so the two render paths keep sharing one SVG string.

Every fill is `currentColor` unless `\textcolor` or `\color` said otherwise,
which is what lets one rendered formula serve twelve themes and follow the OS
light/dark switch without re-rendering.

**The fonts are not embedded.** This application embeds no font it can instead
ship beside the executable; `assets.rs` is the one exception, and a different
case, because gpui requests those two by exact path and diagram labels come out
blank without them. The nineteen KaTeX faces live in `fonts/katex/`,
`package-release.sh` stages them next to the binary, and
`renderer.rs::font_dir_candidates` searches there first. When none of the
candidate directories holds all nineteen, `availability()` reports `Missing`
with an install hint and every formula becomes a diagnostic — the same shape
PlantUML has always used for a missing binary.

**`ratex-parser` is vendored under a `[patch.crates-io]`**, carrying one
25-line clamp. `\begin{alignat}{N}` allocates `N * 2` 64-byte values with no
bound, so a 45-byte document requests 68GB and dies with an allocation abort —
the one failure class the `catch_unwind` in `RendererRegistry::render` cannot
contain. The clamp has to live inside the parser because it macro-expands before
reading the argument: `\begin {alignat}{1e9}` with a space, a comment between
the two, `\def\N{1000000000}...{\N}` and a macro-supplied environment name all
defeat a guard written over the source text. See
`vendor/ratex-parser/README.markturbo.md`.

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

MDX can contain executable code and a local HTML file can reference its own
directory, so the boundary had to be decided now even though no MDX runtime
ships in v0.1.

Everything markturbo *renders itself* is served to the WebView as a `data:` URL
— an opaque origin with no filesystem access, no `file://` reach, and no ambient
credentials — under a CSP where `default-src 'none'` blocks **all** network
access at both trust levels. `Restricted`, the default for every document,
additionally blocks scripts.

Trust is explicit and per-document, and it grants a different power to each of
the two types it applies to:

| | Restricted | Trusted |
|---|---|---|
| MDX | `data:`, no scripts, no network | `data:`, scripts run, still no network |
| HTML | `data:` with an injected CSP | `file://`, loaded from disk |

The HTML row is the actual boundary, and it is deliberate: a trusted `.html` is
loaded through `web::to_file_url` so its relative images and stylesheets
resolve, which is the only reason a user would trust one. That gives it a real
origin, read access to whatever the user can read, and whatever CSP the file
itself carries — which may be none. `to_file_url`'s doc comment says so in those
words, and `only_a_trusted_document_is_given_filesystem_access` pins it.

So the honest summary is not "content can never reach anything". It is: nothing
reaches the filesystem or the network unless the user trusted that specific
document, and for HTML, trusting is exactly the act of handing it the disk.

### Save safety

The filesystem is the source of truth, and agents write to it concurrently. A
save records the file's mtime and size at load; if disk no longer matches, the
save is refused and the user chooses reload or overwrite. Writes go to a
randomly-named sibling temp file and are renamed, so a crash cannot truncate a
document and two concurrent saves of one file cannot collide on one temp path.

Three properties of the bytes survive a round trip: **line endings**, **BOM**,
and **encoding**. The third was the expensive one to get wrong. Reading every
file through `String::from_utf8_lossy` meant a GBK or Shift-JIS document became
a wall of U+FFFD in the editor — and since a save writes the buffer back, one
Ctrl+S replaced the file with those replacement characters irrecoverably. A BOM
now decides the encoding outright, valid UTF-8 is taken at face value, and only
bytes that are neither reach the detector. The encoding travels with the loaded
file so the save re-encodes in it.

`encoding_rs` has no UTF-16 *encoder* — `Encoding::encode` silently emits UTF-8
for those two, which would write a UTF-8 body under a UTF-16 BOM and produce a
file nothing can read. They encode their own code units instead.

### Settings: a file a person opens

Settings are TOML, at whatever `dirs::config_dir()` returns, plus `markturbo`.
Both halves of that were once hand-rolled and both were changed for the same
reason — the file is a user-facing artifact, not an internal cache.

TOML over JSON because a settings file is something people edit: it takes
comments, and it does not fail on a trailing comma. The format imposes one
constraint on the code, which is worth knowing before adding a field: every
scalar must be written before any table, so `AppSettings` must stay flat. A
nested struct or map would serialize to a document `toml` itself refuses to read
back. `the_settings_document_is_flat_enough_for_toml_to_read_back` holds that.

The directory comes from `dirs` rather than four `cfg` branches. Windows and
Linux land where they did; macOS moved from `~/.config/markturbo` to
`~/Library/Application Support/markturbo`, which is the correction — macOS is
not an XDG platform, and a file in `~/.config` there is invisible to every macOS
convention for finding, backing up, or migrating application data.

There is no migration. An existing `settings.json` is not read and not deleted;
the user starts from defaults. That is the standing rule for this stage rather
than an oversight, and the packaged `RUNNING.md` says so where a user will see
it.

`AppSettings::update` is the only writer, and `global_mut` already pushes a
`NotifyGlobalObservers` effect — so the notification was always being sent and
nobody was listening. A single `observe_global` subscription is the backstop for
the writers that are not the settings page.

### Background parsing

`markdown-rs` is superlinear in the *number of blocks*: on this repo's fixtures,
10x the input costs ~65x the time (~13s for 100K lines). This is upstream — the
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
- **A generic plugin platform.** The renderer registry is the extension point
  that was actually needed.

### Reversed: the HTTP client

This section used to say translation shells out to `curl`, and that adding a
TLS stack for one optional endpoint was not worth the build cost. That was a
defensible answer to the question as it stood — one endpoint, one wire format.
The question changed to *many providers, robustly*, and the reasoning did not
survive it.

What the original decision underweighted was not aesthetics. It was this:
`post_json` had **zero test coverage**, in a file with 307 lines of tests.
Everything around it was tested — provider selection, key precedence, endpoint
construction, response shapes — and the one part that talked to the network was
not, because testing it needed a real `curl` binary and a real server. "What
does a 401 turn into for the user?" was unanswerable without going online. A
library client is pointed at a local `TcpListener`, so that question now has a
test.

Two lesser failures came with it. `--max-time 120` is a *total transfer*
timeout, so it cannot distinguish a dead host from a large document that is
legitimately still streaming — and it is uncancellable, so a user who clicked
Translate waited the full two minutes for a typo'd hostname. And the error text
was `curl`'s, not the application's.

The replacement is [`genai`], a multi-provider client covering Anthropic,
OpenAI (both Chat Completions and Responses), Gemini, Ollama, Bedrock, Vertex,
Groq, DeepSeek, xAI, OpenRouter and more — so provider knowledge that used to
live in this repository as hand-written request shapes and response extractors
now lives upstream, where it is maintained.

**The cost, stated plainly:** 41 crates that were not being compiled before,
including `tokio`, `hyper`, `reqwest`, `tower`, `mio`, and `ring`. That is a
real trade and it is bigger than the one this section originally declined to
make.

An earlier draft of this paragraph claimed the async stack was already paid for
— that `tokio`, `hyper` and `rustls` arrived via `gpui-component-assets` and so
cost nothing here. That was wrong, and it is worth recording why, because the
mistake is easy to repeat: `gpui-component-assets` declares `reqwest` only under
`[target.'cfg(target_family = "wasm")']`, so on every target this application
actually builds for, none of it was ever compiled. `cargo tree --target all`
folds the wasm branch in and reports a smaller delta; measuring the target you
ship is the only number that means anything. Measured on
`x86_64-pc-windows-msvc`, the mt-app dependency set goes from 510 to 551.

Two things bound it:

- **No C toolchain.** `genai`'s default feature reaches `reqwest/rustls`, which
  selects `aws-lc-rs` and therefore `aws-lc-sys` — a C library needing `cmake`
  and `nasm` on Windows. Selecting `reqwest/rustls-no-provider` with `rustls`'s
  `ring` provider instead removes every `aws-lc` crate from the graph. The
  trade is that nothing installs a crypto provider automatically, so
  `rustls::crypto::ring::default_provider().install_default()` runs inside the
  same `OnceLock` that builds the client, immediately before `Client::builder()`
  — which is the only ordering that works, because reqwest panics during client
  construction rather than at request time when no provider is installed.
- **No second runtime in the UI.** `genai` is async and this application has no
  async runtime but GPUI's. `TranslationService` therefore stays **synchronous**:
  one shared `tokio` runtime, built with `rt-multi-thread` and a single worker,
  is driven with `block_on` from the background task that already runs the
  translation. Multi-thread rather than current-thread is not a preference: a
  current-thread runtime is driven only by whichever thread calls `block_on`,
  and GPUI hands each background task an arbitrary pool thread. `mt-doc` never
  learns that `tokio` exists, which is the boundary that matters — see the hard
  rule above.

[`genai`]: https://crates.io/crates/genai

## Extension points

Ordered by how cheap they are:

1. **A diagram technology** — implement `BlockRenderer`, register it, add the
   fence language. Nothing else changes.
2. **A skill discovery convention** — one row in `harness::HARNESSES`, which is
   what `skill::discovery_roots` reads. A harness that shares an existing
   directory costs nothing further; dedup collapses the overlap.
3. **A translation wire format** — add a `Provider` variant and map it to a
   `genai::adapter::AdapterKind`. A vendor that speaks an existing format needs
   only a base URL, which is a settings change, not a code one.
4. **A document type** — a `DocType` variant plus its recognition rule.
5. **A split layout** — a `Layout` variant plus its entry in `available_for`.
   `Layout` names its own renderer (`Layout::preview`), so "which pane shows
   what" is answerable from the layout alone rather than from a second control.

## Known limits

- PlantUML needs a local install. Everything else is self-contained.
- The WebView is Windows/macOS only, upstream. Linux gets an explanation in the
  Web pane rather than a broken one; native rendering is unaffected.
- MDX components render as placeholders in both trust levels. The trust boundary
  and CSP are in place for when a runtime ships.
- Live web preview pauses above 512KB; the editor and native preview stay live.
- Closing a tab with unsaved edits discards them without a prompt. The file on
  disk is never touched.
- Translation is the one feature that leaves the machine *on its own*, and only
  when the user invokes it with a key configured. What crosses the wire is prose
  the user asked to have translated. Document content is separate: it cannot
  fetch anything at any trust level — except a trusted HTML file, which is
  loaded from `file://` outside markturbo's CSP and is therefore bounded by the
  browser engine rather than by us. See the trust boundary above.
