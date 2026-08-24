# 01 — The native preview rebuilt its Markdown extensions every frame

Type: task
Status: resolved

## Question

`DocumentView::render_native_preview` calls `diagram_extensions(registry)`
inside `render`. Does that cost anything, and if so, how much?

## Answer

It cost 300-750% of CPU, indefinitely, on any Markdown document over 4 KiB, with
no user input at all.

### The mechanism

`MarkdownExtensions` carries a `revision: u64`. Upstream stamps it from a
process-global counter — `markdown_ext.rs:16` declares
`static MARKDOWN_EXTENSIONS_REVISION: AtomicU64`, and `bump_revision`
(`markdown_ext.rs:302`) does `fetch_add(1, Relaxed)`. Every builder method that
registers something — `block_parser`, `block_renderer`, `push_block_parser`,
`push_block_renderer`, `mdx` — calls it.

`TextViewState::set_markdown_extensions` (`state.rs:268`) short-circuits only
when the revision it already holds equals the incoming one:

```rust
if self.markdown_extensions.revision() == markdown_extensions.revision() {
    return;
}
self.markdown_extensions = markdown_extensions;
if self.format == TextViewFormat::Markdown {
    let text = self.text.clone();
    self.increment_update(&text, false, cx);
}
```

A `MarkdownExtensions` built fresh inside `render` therefore carries a revision
that has never been seen, the guard never matches, and every frame triggers
`increment_update` — a full reparse of the document plus a full pass of the
Renderer Registry over every fence.

Below upstream's 4 KiB `MAX_SYNC_FULL_REPLACE_BYTES` (`state.rs:30`) that parse
runs synchronously and the damage is bounded to wasted work per frame. Above it
the parse is scheduled asynchronously and completes with a `cx.notify()`, which
schedules the next frame, which starts the next parse. That closes the loop.

Note the threshold makes the failure look like a size bug rather than a
structural one: a 3,900-byte document is merely wasteful, and a 4,228-byte
document never stops.

### The fix

Build the extensions once, in `DocumentView::new`, and clone them per frame.
`MarkdownExtensions` derives `Clone`, which copies the revision rather than
minting a new one — so the guard matches from the second frame onward.

The registry is still consulted for rendering; what stops is the rebuild of the
parser/renderer registration, which is what was minting revisions.

### Measured

Release build, 60-second trace, twelve 5-second windows, no user input, one
4,228-byte Markdown document open in the Native layout:

```
BEFORE  538.5  746.8  730.1  713.0  386.5  384.2  312.4  332.2  396.8  425.6  396.7  440.4
AFTER    85.7    0.9    0.3    0.0    0.3    1.2    0.0    0.6    0.6    0.3    0.9    1.2
```

Before never converges. After, the first window is ordinary first-frame render
cost and everything subsequent is zero.

Short-window A/B sampling was tried first and produced contradictory results
(one round showed the fixed binary at 134% and the unfixed one at 9.2%). The
cause was a sampling window landing on startup transients — the harness scan and
the MathJax warm-up both run in the first several seconds. The time series
separates them; a single sample does not. Recorded because the same mistake is
easy to repeat on this machine.

### Guarded by

`the_native_preview_does_not_rebuild_its_extensions_per_frame` in `document.rs`.
A source-level check, because reproducing the failure needs a real window and a
document over the upstream threshold.

Verified red-capable: reverting `render_native_preview` to call
`diagram_extensions` fails the test with the message naming the revision
mechanism.

### Full gate

`cargo test --release --workspace` — 445 passed, 0 failed, including the eight
`mt-doc` performance tests at 110.36s. `cargo fmt --all -- --check` clean.
`cargo clippy` reports the same 4 pre-existing warnings as before the change
(3 `collapsible_if` in `translate.rs`, 1 `while_let_loop` in `watcher.rs`).
