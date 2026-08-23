# markturbo — performance & binary size, measured

Diagnosis run on 2026-08-23, Windows 11 x86_64-pc-windows-msvc, release builds
throughout. Every number below came from a command in this repository; none is
estimated. Where a fix was applied, the before/after are both measured.

---

## Part 1 — Feedback loops

The repo already ships four measurement harnesses, all `#[ignore]`d so they do
not gate CI. They are the loop; nothing here needed inventing:

| Command | What it attributes |
|---|---|
| `cargo test --release -p mt-app --test open_document_cost -- --ignored --nocapture` | load / parse / render per document |
| `cargo test --release -p mt-app --test open_folder_cost -- --ignored --nocapture` | read_dir / watcher / skill+instruction discovery |
| `cargo test --release -p mt-app --test search_cost -- --ignored --nocapture` | walk / query per scope |
| `cargo test --release -p mt-doc --test performance` | parse scaling, the only asserted gate |

Two scratch harnesses were added for costs the existing ones do not isolate,
and are listed for deletion in the cleanup section.

For binary size the loop is `cargo bloat` (installed during this run) plus a
PowerShell PE section dump — `objdump` is not on this machine.

---

## Part 2 — Performance: measured

### 2.1 The dominant cost is MathJax's cold start, and it is already handled

```
cargo test --release -p mt-app --test open_document_cost cold_math -- --ignored --nocapture
cold 872.8ms  warm 53.0ms
warm-up moves 819.9ms off whichever document is opened first
```

`renderer::warm_up()` is spawned from `main.rs:98` before the window opens, so
the 873ms overlaps window creation instead of landing on the first document
with a formula. The mechanism is sound: `mathjax-svg-rs` builds its Boa engine
behind a `OnceLock` on a dedicated worker thread with a 4 MiB stack, so a real
render concurrent with the warm-up waits for it rather than building a second
engine.

**The measured cost is higher than the code says.** `renderer.rs:203` documents
"~640ms" and `warming_up_returns_immediately_and_leaves_math_working` repeats
it; this machine measures **873ms cold / 53ms warm**. The comment is stale by
about 230ms, not wrong in kind.

### 2.2 Opening a document: rendering dominates, everything else is noise

```
cargo test --release -p mt-app --test open_document_cost attribute -- --ignored --nocapture
    load    parse   render    total  document
 375.4µs  681.6µs  100.0ns    1.1ms  SKILL.md (526 bytes, 0 fences)
 292.2µs    1.8ms     1.0s     1.0s  README.md (3386 bytes, 2 fences, slowest math 1.0s)
 594.1µs    1.1ms  218.5ms  220.1ms  diagrams.md (2274 bytes, 10 fences, slowest math 102.1ms)
```

Read and parse are sub-millisecond on real documents. The whole cost is
diagram/math rendering, and within that it is math: 1.0s on README.md is the
cold engine start being paid by whichever document happens to be first in the
walk. `diagrams.md` at 218ms for 10 fences is the warm steady state.

The registry caches by `(id, source)` (`renderer.rs:131`), so scrolling and
re-rendering are free — the cache is load-bearing, correctly commented as such.

### 2.3 Global skill discovery is the one real UI-visible stall

```
MARKTURBO_BENCH_DIR=Q:/repos/markturbo cargo test --release -p mt-app --test open_folder_cost -- --ignored --nocapture
     248µs  read_dir (depth 0)  16 entries
     765µs  read_dir (depth 1)  16 entries, 20 children
     861µs  Watcher::new (recursive)  ok
       1ms  skill::discover (workspace)  0 skills
     444ms  skill::discover_with (global)  213 skills
       2ms  instruction::discover  0 files
      41ms  instruction::discover_with (global)  4 files
```

444ms, against sub-millisecond for everything else. Attributed further with a
scratch harness:

```
70 global roots resolve on this machine
   177.4ms    369 dirs  C:\Users\lhan\AppData\Local\hermes\skills
    33.8ms     39 dirs  C:\Users\lhan\.copilot\skills
    30.4ms     92 dirs  C:\Users\lhan\.agents\skills
    24.1ms     67 dirs  C:\Users\lhan\.claude\skills
    23.9ms     62 dirs  C:\Users\lhan\.pi\agent\skills
    13.6ms     32 dirs  C:\Users\lhan\.codex\skills
    ...59 more roots at ~50-90µs each (they do not exist)
discover_with(everything): 286.7ms  213 skills
canonicalize x213: 21.1ms
read SKILL.md x213: 75.2ms
parse x213: 5.2ms
```

So the 444ms is: ~306ms walking directories, ~75ms reading 213 `SKILL.md`
files, ~21ms canonicalizing, ~5ms parsing YAML. It is filesystem-bound, and it
is dominated by one root with 369 directories under it.

**This is correctly off the UI thread** — `harness.rs:113` runs it in
`background_spawn` with a replace-to-cancel task slot, so a burst of watcher
events costs one scan. What it is *not* is incremental: every filesystem event
matching `skill|agents.md|claude.md|instructions.md|rules`
(`workspace.rs:656`) re-runs the whole 444ms scan, including 70 root
resolutions and 213 file reads, to discover that nothing changed.

The `SPINNER_FLOOR` in `harness.rs:116` exists because the *workspace* scan is
fast enough to flash; the global scan is 444ms and needs no floor at all.

### 2.4 Search: the walk is cheap, the query is not

```
MARKTURBO_BENCH_DIR=Q:/repos/markturbo cargo test --release -p mt-app --test search_cost -- --ignored --nocapture
walk:    44.0ms  (1075 documents)
query                  the:  109.5ms  500 match(es) in 43 file(s) [capped]
query            workspace:  459.1ms  500 match(es) in 77 file(s) [capped]
query   zzzz-no-such-token:  424.5ms  2 match(es) in 1 file(s)
```

The rare needle is the honest worst case — nothing stops it early, so it reads
every one of 1,075 documents: 424ms. Behind the 250ms debounce
(`search.rs:99`) and on a background task, so it does not freeze the window,
but it is 424ms of full-corpus I/O per settled query.

`search_text` allocated a lowercased `String` **per line** for the default
case-insensitive query, and — for no reason at all — a `line_text.to_string()`
per line in the case-*sensitive* branch, which needs no copy whatsoever. On a
100K-line document that is 100K allocations per file. Isolated:

```
1516435 bytes, 100000 lines
lowercase every line: 6.6ms
ascii in-place scan:  3.7ms
search_text (rare needle):    13.0ms
search_text (case-sensitive): 10.9ms
```

**Applied:** `haystack` is now a `Cow<str>`. Case-sensitive borrows outright;
case-insensitive borrows when the line is ASCII with no uppercase, which is the
overwhelming majority of lines in the documents this app opens. Only a line
that actually changes under `to_lowercase` allocates.

### 2.4b Per-renderer cost, warm engine

```
        d2:   60 blocks    424.2ms total     7.1ms each
      math:   60 blocks       1.8s total    29.8ms each
   mermaid:   60 blocks      6.3ms total   104.2µs each
cold pass total: 2.2s
warm (cached) pass total: 277.6µs
```

Mermaid is essentially free. D2 is 7ms a diagram. Math is 30ms a formula even
warm, and 60 formulas is 1.8 seconds — which is why the registry cache
(`renderer.rs:131`) is load-bearing rather than an optimization: the second
pass over the same document is **277µs**, four orders of magnitude cheaper.

### 2.5 Parse scaling: healthy, and the gate proves it

```
cargo test --release -p mt-doc --test performance
8 passed; 0 failed  (143.92s)
```

All eight pass, including `the_engine_adds_no_superlinear_overhead_over_the_parser`,
which measures the engine's overhead ratio at 10K and 100K lines rather than
comparing growth ratios. That test alone takes ~130 of the 144 seconds
(best-of-three over a 1.4MB fixture, four times).

markdown-rs itself is superlinear in block count — the comment at
`performance.rs:73` records 10× input ≈ 70× time and correctly localizes it
upstream. The engine adds nothing on top.

`schedule_reparse` (`document.rs:601`) runs the reparse on a background
executor with a stale-result check, and `set_source` short-circuits an
identical source. Both are right.

### 2.6 Per-frame work in `render`

`DocumentView::render` calls `self.title(cx)` (`document.rs:1226`), and for a
buffer not on disk `title` calls `self.text(cx)` — which is
`EditorState::value()`, and upstream that is
`SharedString::new(self.text.to_string())` (`state.rs:1099`): **a full copy of
the rope, per frame**. On a file with a path this is not reached (the early
return at `document.rs:222` uses the file name), so it only bites untitled
buffers — but there it is a whole-document clone at 60fps.

`Workspace::render_tabs` (`workspace.rs:1375`) calls `doc.title(cx)` for
**every open tab**, every frame, with the same reachability.

`sync_preview_scroll` is also called from `render`, but it is correctly
self-debounced by `synced_row` (`document.rs:317`) before anything expensive
runs. That one is fine.

### 2.7 `to_data_url` allocated per byte  ✅ FIXED

`web.rs`: `other => out.push_str(&format!("%{other:02X}"))`. A `format!` is a
heap allocation; for HTML the great majority of bytes fall in that arm. The
`with_capacity(len * 2 + 32)` was also short — a fully-encoded byte is 3
characters, so the buffer reallocated as well.

```
payload: 293204 bytes
to_data_url: 17.4ms   allocation-free: 730.5µs   ratio 23.9x
at the 512KB live-preview limit: 17.6ms
```

And in context, per Web-pane rebuild:

```
      large-10k.md: build  168.2ms  encode   17.5ms  html 293204 bytes  url 537167 bytes
  diagram-heavy.md: build    1.7ms  encode   27.2ms  html 980038 bytes  url 1348033 bytes
```

On the diagram-heavy document the *encode* cost 27ms against 1.7ms to build
the HTML — the encoder was 16x the thing it was encoding.

**Applied:** a two-character hex table (`push_percent`), used by both
`to_data_url` and `to_file_url`, plus a correctly-sized capacity. 24x on the
measured payload.

---

## Part 3 — Binary size: measured, and cut

### 3.1 Baseline

```
target/release/markturbo.exe   82,848,000 bytes  (79.0 MiB)
.rdata  47,887 KB
.text   35,447 KB
.pdata   1,160 KB
.data      248 KB
.reloc     170 KB
.rsrc        2 KB
```

`.rdata` being larger than `.text` is the tell: this is not code, it is tables.

### 3.2 Where the code is

```
cargo bloat --release --bin markturbo --crates -n 40
 5.6%  13.5%   4.7MiB gpui
 4.8%  11.5%   4.0MiB std
 3.5%   8.4%   2.9MiB boa_engine      <- the JS engine MathJax runs on
 2.6%   6.2%   2.2MiB d2_little
 2.5%   6.0%   2.1MiB gpui_component
 1.8%   4.2%   1.5MiB mermaid_svg
 1.8%   4.2%   1.5MiB boa_parser
 1.0%   2.5% 869.0KiB usvg
 0.8%   1.9% 664.2KiB mt_app          <- this project
 0.7%   1.7% 588.8KiB genai
 0.6%   1.5% 533.6KiB image_webp      <- unreachable: no WebP is ever decoded
 0.5%   1.3% 473.4KiB image
 0.5%   1.2% 424.3KiB zune_jpeg
 0.3%   0.8% 292.9KiB exr             <- OpenEXR. In a Markdown editor.
41.7% 100.0%  34.6MiB .text section size, the file size is 82.9MiB
```

`mt_app` is 664 KiB of a 79 MiB binary — 0.8%. Nothing this project wrote is
the problem.

Top individual symbols are dominated by `ts_lex` (tree-sitter generated
lexers), one of which alone is 324.9 KiB, and by
`gpui_component::theme::schema::…::deserialize::visit_map` at 275.9 KiB —
serde's generated deserializer for the theme schema.

### 3.3 The finding: 34 tree-sitter grammars, for a Markdown editor

`Cargo.toml:24` enabled `tree-sitter-languages`, which is gpui-component's
"everything" feature — 34 grammars, each a generated C parse table:

```
tree-sitter-kotlin-sg   5,936 KB static lib
tree-sitter-c-sharp     5,496 KB
tree-sitter-swift       4,172 KB
tree-sitter-scala       3,780 KB
tree-sitter-cpp         3,680 KB
tree-sitter-typescript  3,208 KB
tree-sitter-sequel      2,776 KB
tree-sitter-php         2,568 KB
tree-sitter-ruby        2,272 KB
...
```

Those tables are `.rdata`, which is exactly the section that dwarfs `.text`.

**Applied:** replaced `tree-sitter-languages` with eleven named grammars —
markdown, rust, bash, toml, yaml, python, javascript, typescript, html, css,
make.

```
before  82,848,000 bytes   .rdata 47,887 KB   .text 35,447 KB
after   54,269,952 bytes   .rdata 17,161 KB   .text 34,228 KB
        -28,578,048 bytes  (-34.5%)
```

**34.5% off the shipped binary, one manifest edit, no code change.**

#### Why eleven and not one — the correction

The first version of this section justified the list as "what files this app
opens", and that reasoning was **wrong**. Challenged with "markturbo is
Markdown-only, why highlight other languages at all?", the honest answer turned
out to invert the argument.

A ``` fence *inside a Markdown document* is highlighted through the same
grammars as a source file:

```
README.md containing ```rust
  -> CodeBlock::styles()                          (ui/src/text/node.rs:1217)
  -> SyntaxHighlighter::new("rust")               (highlighter.rs:336)
  -> LanguageRegistry::singleton().language(...)  (registry.rs:526)
  -> Language::from_name("rust")                  (languages.rs:178)
       #[cfg(feature = "tree-sitter-rust")]
```

`LanguageRegistry::singleton()` is built from `Language::all()` — the same
cfg-gated enum (`registry.rs:502`). So dropping `tree-sitter-rust` does not
merely stop highlighting `.rs` files; it renders every ```rust block in every
README as flat grey text. Fence languages counted across this repository's own
Markdown:

```
8606 ```rust      77 ```bash     23 ```toml    6 ```json
  73 ```mermaid   57 ```text     21 ```sh      4 ```yaml
  64 ```d2        36 ```rs        6 ```math    4 ```diff
```

That is the list. It was chosen correctly by accident and justified wrongly.

**Markdown-only was built and measured, then rejected:**

```
markdown-only   49,107,456 bytes   (a further -5.2MB)
```

Rejected on two counts. It un-highlights every fence — the opposite of what a
Markdown workspace wants — and it does not compile: the grammar feature gates
the `Language` enum *variant*, not just its highlighting, so `Language::Rust`,
`::Toml`, `::Make` and friends cease to exist. Five `E0599`s in `document.rs`.

That cfg behaviour is the trap. `SyntaxHighlighter::new` degrades gracefully at
*runtime* (logs, builds an inert highlighter, never panics), which makes it easy
to assume a removed grammar is a soft downgrade. At *compile* time it is a hard
error wherever the variant is named.

**Guarded:** `every_fence_language_used_in_documents_has_a_grammar`
(`document.rs`) asserts each fence language above resolves to a real grammar,
and that `text` still resolves to `Plain` so the assertions cannot go vacuous.
Verified red-capable — removing `tree-sitter-css` from the manifest fails the
build with `no variant ... named `Css``, naming the exact language lost.

Two more notes for anyone editing the list: `tree-sitter-json` is *not* a
feature name (JSON arrives via the base `tree-sitter` feature that every grammar
feature enables), and `tree-sitter-make` is kept for
`a_file_with_no_grammar_falls_back_to_plain`, which names `Language::Make`.

`tree-sitter-tsx` is the one addition that would cost **0 bytes** — it reuses
the `tree-sitter-typescript` parse table already compiled in. Not added, since
the goal here was shrinking rather than restoring.

### 3.4 `lto = "fat"`: measured and rejected

```
lto = "thin"  54,269,952 bytes   5m 21s
lto = "fat"   53,003,264 bytes  15m 24s
```

1.2 MB (2.3%) for triple the build time. Reverted to `thin`. Recording it so
nobody re-runs the experiment.

### 3.5 What is left, and what it would cost

| Item | Size | Removable? |
|---|---|---|
| `boa_engine` + `boa_parser` + `boa_ast` | ~4.9 MiB `.text` | Only by dropping MathJax. It is the only pure-Rust math path that emits glyph outlines, which resvg needs. Keep. |
| MathJax JS bundle | 1.5 MB source, zstd-compressed into `.rdata` | Same. Keep. |
| `image_webp`, `exr`, `zune_jpeg`, `image` | ~1.7 MiB | Pulled by `gpui`'s `image` dependency, not selectable from here — gpui does not expose per-format features. Would need an upstream patch. |
| `webview2-com-sys` build output | 31 MB in `target/`, small in the binary | Not shipped. Ignore. |
| 213 KB of embedded fonts (`assets/fonts`) | 385 KB | Required: gpui's SVG renderer asks for exactly these two, and without them every diagram label renders blank. Keep. |

---

## Part 4 — Findings, ranked

### F1 — 34 tree-sitter grammars shipped for a Markdown editor  ✅ FIXED
82,848,000 → 54,269,952 bytes, **-34.5%**. One manifest edit, no code change.
See §3.3 for why the list is eleven and not one — the reasoning in the first
draft of this report was wrong, and the correction is load-bearing: fence
highlighting inside Markdown documents runs on these same grammars.

### F2 — Global skill discovery re-scans everything on every relevant fs event
**Open.** 444ms, 70 roots, 213 file reads, per event that matches a broad
substring test (`workspace.rs:656`: any path containing `rules` re-triggers
it). Correctly backgrounded with a replace-to-cancel task slot, so it is not a
freeze — but a `.cursor/rules/x.md` save costs a full re-walk of
`AppData\Local\hermes\skills`. The scan has no mtime check and no per-root
invalidation.

Cheapest real fix: skip a global root whose directory mtime is unchanged since
the last scan. 59 of the 70 roots do not exist at all and are re-resolved from
scratch every time.

Not applied here: it changes discovery semantics (a skill edited *inside* an
unchanged directory would need the mtime check to reach the file, not just the
root), which is a correctness decision rather than a mechanical speedup.

### F3 — `search_text` allocated per line, twice over  ✅ FIXED
The case-sensitive branch's `line_text.to_string()` was pure waste. Now a
`Cow<str>` that borrows for case-sensitive queries and for ASCII-lowercase
lines, allocating only when `to_lowercase` actually changes the line.

### F4 — `title()` clones the whole rope per frame for untitled buffers
**Open.** `document.rs:231` → `text(cx)` → `EditorState::value()` →
`SharedString::new(self.text.to_string())`. Reached from both
`DocumentView::render` and `Workspace::render_tabs`, the latter once per open
tab per frame. Only affects buffers with no path, which bounds the damage.

Not applied here: upstream `EditorState` exposes no first-line accessor
(`state.rs:1123` returns `&Rope`, but that method is on a different type than
the one `DocumentView` holds), so the fix is either an upstream addition or a
cached title invalidated on edit. Both are more than a one-line change.

### F5 — `to_data_url` heap-allocated per encoded byte  ✅ FIXED
24x on a 293 KB payload (17.4ms → 730µs). Both `to_data_url` and `to_file_url`
now share a `push_percent` helper with a static hex table, and the capacity
hint is `len * 3` rather than `len * 2`.

### F6 — The warm-up comment was 230ms stale  ✅ FIXED
`renderer.rs`, its test, and `main.rs:96` all said ~640ms; measured 873ms.
Corrected to ~870ms / ~50ms in all three places.

### F7 — `lto = "fat"` is not worth it  ✅ MEASURED, REJECTED
1.2 MB for triple the build time. Documented so it is not re-litigated.

---

## Part 5 — What was verified

- `cargo fmt --all -- --check` — clean
- `cargo clippy --workspace --all-targets` — 4 warnings, all pre-existing
  (3 `collapsible_if` in `translate.rs`, 1 `while_let_loop` in `watcher.rs`);
  the fixes above introduce none
- `cargo test --release --workspace` — see the run at the end of this session
- `cargo build --release` at each configuration, sizes read off disk
- PE sections read directly from the binary, not estimated

## Part 6 — Cleanup

Four scratch harnesses were written to attribute costs the shipped ones do not
isolate (per-root discovery, per-renderer, lowercase allocation, data-URL
encoding). All four have been **deleted**; their numbers are quoted above, and
the four permanent harnesses in Part 1 remain the loop for future work.

No `[DEBUG-…]` instrumentation was added at any point — every measurement came
from a test harness, so there is nothing to grep for.
