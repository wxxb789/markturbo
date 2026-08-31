# Architecture

The boundaries chosen for v0.1, and why. This records the architecture work
requested by the [historical v0.1 product direction](history/v0.1-product-direction.md),
after inspecting upstream source rather than relying on assumptions. Current
product scope is governed by the [Product Contract](../PRODUCT.md).

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

Inspecting upstream source first, as the historical v0.1 brief required, changed
the plan substantially.
`gpui-component` already ships:

- `MarkdownExtensions` — a block parser/renderer registry with an MDX parse
  mode. The renderer registry requested by the historical brief at the *view* layer already
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
loaded file has a `FileStamp`; before a normal Save, the current named object
is opened and its metadata, bytes, and SHA-256 are collected from that same
handle. This detects a changed document even when its mtime and length happen
to match the loaded version. Windows also records the file object ID, so a
delete-and-recreate at the same path is a conflict even if the replacement has
the same bytes and timestamps. A mismatch refuses the normal Save and the user
chooses reload, overwrite, recreation, conversion, or Save As as the case
requires.

Those choices are explicit capabilities, not independent `bool` flags. A
`SaveAuthorization` begins as a normal Save; an overwrite captures the exact
current `FileStamp` of the resolved destination, and a missing-file decision
records permission to recreate only a currently absent regular file. UTF-8
conversion composes with either source decision without replacing it. Before
commit the save rechecks that exact expected stamp, or that the approved path
is still absent. Therefore a later external writer, or a path that reappears
after a recreation decision, becomes a new conflict rather than inheriting an
earlier answer. The document keeps that authorization only until an edit,
successful save, source-identity change, or new conflict resets it.

The replacement output is staged in a randomly named sibling file, synced, and
then installed with guarded `ReplaceFileW` handling on Windows. The backup is
verified against the expected outgoing object before the replacement is
accepted. If a race cannot be proved safe, the editor remains dirty and reports
the preserved artifacts rather than silently treating the Save as successful.
Win32 is not compare-and-swap and the filesystem is not universal version
history; the implementation detects and contains the races it can establish,
rather than claiming an impossible guarantee for every concurrent actor.

Saving through a supported symbolic link preserves the link itself and updates
its resolved target. The watcher maps changes observed on that resolved target
back to documents opened through the link.

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

### Destructive lifecycle and recovery

Closing a dirty tab, closing the window, and replacing the workspace all pass
through one lifecycle decision. The decision walks every affected dirty buffer
and requires **Save**, **Discard**, or **Cancel** before the destructive action
can proceed. A Save must succeed before its buffer is released. The lifecycle
model also represents memory-origin documents, so a future new-document flow
uses the same boundary rather than a separate close path.

The destructive coordinator revalidates the affected documents immediately
before destruction. If another document becomes dirty, or a prompted document's
revision changes while a prompt is open, it is returned to the decision queue
and prompted for its current revision before the destructive action proceeds.

Asynchronous text changes have a different boundary. A `BufferSnapshot`
captures the editor revision and exact text used by destructive lifecycle
decisions. An `AsyncSnapshot` adds a source generation that advances after a
successful Save As. Translation and background reparsing use this stronger
identity: a result may land only when the revision, text, and source generation
still match. An edit or Save As while work is in flight therefore leaves the
current document untouched and asks the user to run the operation again.

Recovery is optional application state, not a workspace format and not an
autosave of the source file. On Windows, encrypted checkpoint records live by
default in `%LOCALAPPDATA%\markturbo\recovery`; `MARKTURBO_DATA_DIR` redirects
the application-data root, making the candidate recovery location
`%MARKTURBO_DATA_DIR%\recovery`. The override must be absolute. Recovery storage
accepts only local fixed, removable, or RAM volumes: UNC paths, mapped/remote
drives, and every unsupported drive type are rejected before storage creation.
It revalidates the nearest existing ancestor before creation and the canonical
volume after creation. WebView and log paths may still use the absolute data
override; an unavailable recovery store is reported while editing and
source-file Save continue. DPAPI protects each record for the current user, and
a sibling temporary file plus sync and rename makes each completed checkpoint
atomic. Non-Windows builds deliberately have no plaintext fallback.

Workspace construction opens the requested folder and file before it starts
recovery. A deferred task opens the store, decrypts and verifies records, and
parses recovered Markdown on the background executor; the result returns
through the same fallible window-update path used by other async UI work. A
document edited while that work runs still creates checkpoint timing state
immediately. If its two-second dispatch deadline arrives before the store is
ready, the workspace keeps a persistent protection warning. When the store
arrives, it attaches the generation token without moving the original edit or
ten-second durable deadline. A matching tab accepts recovery only if it is still
clean and has not advanced
from the identity captured when startup began. A clean file opened while the
scan is running may also accept its matching record, while any edit or reload
keeps the current tab untouched. Existing watcher conflict state is preserved.
Watcher events still mark affected documents during this interval, but startup
recovery is also an auto-reload barrier: automatic reload remains off until the
scan has completed, even when the setting is enabled.
Successful Save and confirmed Discard decisions made before the store is ready
queue their recovery keys. The startup result filters those keys before it
restores records. Once it has attached an available store, it durably retires
the queued keys and only then resumes a waiting destructive action. If the
store remains unavailable, the document stays open and the failure is reported;
editing and source-file Save remain available. Restored dirty records are
already durable, so they do not immediately rewrite their checkpoint; they are
instead scheduled for a ten-second refresh from their restored durable baseline.

The Windows production store validates the local-volume policy before it
creates the root, then takes lifetime ownership before it opens any record. It
holds a directory handle opened on the canonical root itself, rejects a final
reparse point, verifies the root's stable file-object ID after canonicalization
and before each operation, and retains an exclusive `.markturbo-recovery.lock`
handle. The lock deliberately rejects another markturbo instance instead of
letting two processes coordinate the same recovery directory. That instance
starts with recovery unavailable, reports why, and continues with editing and
source files intact. This is a recovery-storage interlock, not a lock on a
workspace or source document.

After an edit, a single earliest-deadline scheduler tracks the oldest edit not
covered by a durable checkpoint. It dispatches after two seconds without an
edit, but continuous edits cannot postpone that oldest uncovered edit beyond
the same two-second dispatch budget. Each attempt retains the exact snapshot
time and an absolute durable deadline: at most eight seconds after dispatch and
never later than ten seconds after the oldest edit it covers. Periodic refresh
is anchored to the durable baseline rather than completion, so a slow worker
cannot add a second ten-second delay. The background batch records
`store_returned_at` as soon as the store returns; deadline success or failure is
judged against that timestamp, rather than a later UI callback.

All documents due on one wake-up enter one ordered physical batch, while each
attempt owns its cancellation flag. A later edit, Save, Discard, or
source-identity change cancels only that stale snapshot without taking the
recovery mutation lock; other due documents continue to a durable checkpoint
instead of being starved by activity elsewhere. Protection and retention work
observe cancellation between bounded waves and before publication. A retention
scan stops early only when every current attempt is cancelled. A transaction
journal is the durable linearization boundary: cancellation before it prevents
publication, while a transaction that has crossed it finishes under the normal
transaction rules. There is only one physical checkpoint worker per workspace.
If it remains occupied past a logical attempt's deadline, the editor reports
the protection warning and retains the latest due schedule rather than spawning
an independent retry. Once that batch returns, the worker slot is released and
the scheduler immediately coalesces repeated edits into one follow-up batch for
the latest snapshot. A late or stale completion cannot clear a warning for a
newer revision; only a current successful checkpoint can do so.

The store verifies the recovery root, takes the mutation lock, and performs one
retention scan for each successful batch. DPAPI decode and checkpoint protection
use bounded parallel waves of at most four workers per stage; when those stages
overlap, their combined active work is capped at eight calls. Each wave is also
limited by the store byte bound. Quota decisions and durable writes remain
serial and preserve dispatch order.

Each request carries a generation token, so a checkpoint already in flight
cannot recreate a record after Save or Discard retires it. Capability state
(generations, active keys, and pending retirements) uses a short lock independent
from the recovery-root I/O lock. After a successful Save or confirmed Discard,
the destructive action writes and syncs a durable retirement marker before it
destroys the document or window. That marker is the non-restorable linearization
point: recovery hides every marked canonical record across interruption, while
background cleanup serializes with checkpoint I/O and retries after failure. A
post-persist marker-sync failure is retried by opening, decoding, and syncing
the exact existing marker; a different marker is rejected rather than replaced.
A multi-document destructive action writes one versioned batch marker, so all
confirmed Discard decisions become durable together or none do. Startup
reconciliation removes marked canonical records before deleting the marker;
cleanup failure leaves the marker in place and reports the condition instead of
exposing retired text. An unreadable, unsupported, or path/key-misbound marker
fails recovery closed rather than exposing a possibly retired record; that
recovery failure does not make editing or source-file Save unavailable.

The workspace keeps action-scoped queued intent separate from durable cleanup
ownership. `pending_recovery_retirements` records the originating document when
known, so a tab-close action waits only for its own pending key; an unknown
origin remains fail-closed for every destructive action. A pending entry means a
newer Save or Discard still needs its own durable marker; it is not an owner.
`recovery_retirements` and
`recovery_retirement_batches` hold the exact single-key or batch ticket that
owns cleanup already made durable. An owner and queued intent for the same key
therefore mean that a later decision is waiting behind the current owner. When
that exact owner finishes, the workspace replays the queued intent to publish a
fresh marker. An edit cancels only the queued intent, while the durable owner
continues cleanup. Completion callbacks compare the complete ticket identity,
so a stale callback cannot remove or replace a newer owner.

A destructive action remains open while any relevant queued intent lacks a
durable marker. If an existing owner or retry must finish first, the action
resumes through the retirement continuation and rechecks the full key set; it
proceeds only after every relevant queued intent has acquired a durable owner.

The store registers active recovery keys under the capability lock. When it
chooses an eviction candidate it combines those live keys with the checkpoint
dispatch snapshot. A document that becomes dirty after work was queued is
therefore not treated as inactive merely because the worker has an older view of
the tab set. Before an eviction transaction performs recovery-root I/O, it
reserves every victim under that capability lock and then releases the lock. A
document activated while its record is reserved immediately receives a visible
protection warning and waits for a later checkpoint; it is never evicted as
inactive. An active or already-reserved candidate defers the transaction.

Retention is capped at 50 records, 32 MiB per record, and 128 MiB total;
records expire after seven days. Startup and completed checkpoint batches prune
expired records. Retention scans keep only the key, timestamp, path, and byte
size after validating a record; full recovered text and save metadata remain in
memory only for an actual recovery scan. A current same-key replacement first
accounts for the old file's metadata, count, and bytes, then treats it as
non-evictable without decrypting stale ciphertext that the new atomic write will
replace. Any real preparation or persistence failure falls back to a full scan,
so malformed and expired data is still reported and pruned; normal cancellation
does not pay that fallback cost before the latest text can catch up. A same-key
replacement that needs no eviction uses one atomic prepared-file replacement.
A write that evicts another record keeps the recovery transaction. Active
records are never eviction candidates, and a quota failure is visible to the
user. Retention scans do not read source documents; the recovery scan still
reads the source as needed to compute restore conflicts.

Replacing a checkpoint or evicting records is a small recovery-store
transaction. The store writes a validated journal, stages the affected old
records, publishes the new record, then writes a commit marker before cleanup.
On startup and before maintenance, it reconciles any journal: a published
transaction is finished (including the case inferred from its on-disk state),
while an uncommitted one is rolled back. If a fault leaves ambiguous or
conflicting artifacts, recovery reports the condition rather than presenting
them as ordinary records. The journal protects recovery retention only; it does
not promise a multi-file transaction for user documents.

After a successful Save or confirmed Discard, the matching recovery record is
durably retired before long physical cleanup. Recovery
stores the source path, encoding, BOM, line endings, conflict stamp, source
identity, and decode-error state with the text. On restore, a changed,
unreadable, or missing source remains a conflicted dirty buffer and cannot
overwrite implicitly. Malformed, oversized, expired, unreadable, unavailable,
or failed recovery data is reported without blocking the editor or modifying a
workspace file. At startup, valid file-backed records from the process-global
per-user store are restored into the startup window without filtering their
source paths against `self.root`. Tab deduplication compares the stored paths
lexically, so equivalent canonical, relative, or link spellings can still be
distinct. A record from another workspace can therefore appear in that startup
window; matching-workspace recovery scoping does not exist yet. In-memory record
restoration is reserved for the Goal 03 new-document flow.

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

Per the historical v0.1 non-goals: no knowledge graph, backlinks, canvas, PKM system, proprietary
format, WYSIWYG, collaboration, sync, accounts, plugin marketplace, extension
runtime, terminal, debugger, LSP IDE, git client, vector DB, RAG, or AI chat
sidebar. Agents live in Codex, Claude Code, Cursor, and the terminal; this app
owns the workspace they consume.

Also not built, and why:

- **A dock/panel system.** The workspace owns one two-row, three-track frame:
  title and body resolve the same retained, user-owned panel widths, while the current
  window width only clamps that render. Boundary handles update those owned
  widths directly. A nested `h_resizable` would introduce a second, previous-
  frame geometry source and can let title and body diverge after a window-state
  transition. `DockArea` remains available if freeform panels are ever wanted.
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
- Translation is the one feature that leaves the machine *on its own*, and only
  when the user invokes it with a key configured. What crosses the wire is prose
  the user asked to have translated. Document content is separate: it cannot
  fetch anything at any trust level — except a trusted HTML file, which is
  loaded from `file://` outside markturbo's CSP and is therefore bounded by the
  browser engine rather than by us. See the trust boundary above.
