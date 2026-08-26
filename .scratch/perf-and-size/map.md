# Performance and size

## Destination

markturbo starts fast, never blocks the UI, and pays for a component only when
that component is used. Concretely, on `x86_64-pc-windows-msvc`: idle CPU at
rest is zero, empty-workspace startup working set is under 80 MB, and the
release binary is under 40 MB — with image display, translation, and all four
block renderers intact.

The 30 MB memory target the effort started from is retained as a north star and
was measured to be below GPUI's own floor; see the decisions below.

## Notes

Domain: a native GPUI desktop application. `CONCEPTS.md` is the glossary — Block
Renderer, Renderer Registry, Native Renderer, Web Renderer, Document Engine.
`docs/architecture.md` records the boundaries and the reasoning behind each
dependency; the root `Cargo.toml` carries the same for every crate selection.
`docs/perf-and-size-2026-08-23.md` is the prior measurement pass.

Standing preferences for this effort:

- **Measure, never estimate.** Every number recorded here came from a command
  run on this machine. This machine is noisy: a concurrent release build has
  been observed to change the same measurement by 2x. Take a time series rather
  than a single sample, and re-measure on a quiet machine before concluding.
- **This machine has no GPU** — WARP software rasterization, `d3d10warp.dll`.
  Memory that would live in VRAM on real hardware counts as process private
  bytes here. Every memory number below carries that caveat.
- **One ticket, one commit**, with the measured before/after in the commit body.
- Windows only. macOS and Linux are deliberately deferred; see Not yet specified.
- Skills to consult: `grilling` and `domain-modeling` for any ticket that
  changes vocabulary or a user-visible contract; `research` for anything needing
  facts from outside this working directory.

Fixed constraints, set by the user and not open for re-litigation:

- Image display is required.
- Translation is required.
- Math rendering stays enabled by default, with a settings toggle to disable it.

## Decisions so far

- [01 — The native preview rebuilt its Markdown extensions every frame](issues/01-per-frame-markdown-extensions.md):
  `render_native_preview` called `diagram_extensions` inside `render`, minting a
  new process-global revision each frame and defeating upstream's guard. Fixed
  by building the extensions once in `DocumentView::new` and cloning per frame.
  Measured over 60s with no user input on a 4,228-byte document: 300-750% CPU
  before, 0-1.2% after.
- [02 — Replace MathJax with RaTeX, lazily, embedding no fonts](issues/02-ratex-replaces-mathjax.md):
  RaTeX minus `ratex-svg` — its `standalone` feature is the only route to
  `<path>` output and it reaches `ratex-unicode-font` through two edges, so the
  SVG is emitted in `renderer.rs` instead. Deletes an `eprintln!` past the `log`
  crate, five hardcoded distro font paths, and a 46 MB system font that was
  never freed. `ratex-parser` is vendored with a 25-line clamp: an unbounded
  `\begin{alignat}{N}` allocation aborts, which `catch_unwind` cannot contain.
  Measured: empty-workspace RSS 148.8 MB → **81.9 MB**, binary 54,310,400 →
  **46,766,080** bytes, per formula ~146 ms → 0.50–0.92 ms, KaTeX corpus 87/90 →
  **89/90**. Also fixes `\mathbb`, `\mathcal`, `\mathfrak`, `\mathsf`,
  `\mathtt`, `\ell`, `\Re` and `\Im`, which MathJax failed permanently.
- [03 — Keep the WebView in one window without native-overlay conflicts](issues/03-webview-covers-overlays.md):
  the WebView stays in the main window but moves to a dedicated STA worker and
  private `WS_CHILD` host. Web mode uses fixed, overlay-free chrome and does not
  offer `SplitWeb` on Windows. Runtime proof found one application top-level
  window, a visible bounded `WRY_WEBVIEW`, 6/6 resize transitions, clean
  shutdown, and zero `RefCell already borrowed` lines.
- [04 — Incremental global skill discovery](issues/04-incremental-global-skill-discovery.md):
  global roots are cached independently with visited-directory and entry-file
  stamps, while workspace roots stay live. Measured over 257 skills: cold
  median 268 ms, warm median 64 ms (76.12% lower).
- [05 — Measure `opt-level = "s"`](issues/05-opt-level-s.md):
  fresh builds confirmed a 20.65% size reduction. The host never passed the
  pre-registered quiet-machine gate during a one-hour rolling wait, so the
  formal startup and formula A-B-B-A comparison did not run. Runtime impact
  remains inconclusive and `s` is not adopted.

## Not yet specified

- **CJK inside math on macOS and Linux.** Math glyphs are `<path>` outlines from
  the bundled KaTeX faces and so are platform-independent, but a CJK character
  inside `\text` falls through to `<text>` and is resolved by usvg against the
  font database gpui populates from the system. On Windows that was measured
  working (`Fallback from Arial to DengXian`). What the other two platforms
  resolve it to, or whether they resolve it at all, is untested.

- **macOS and Linux.** Every number in this effort is from one Windows machine
  with no GPU. Whether the memory floor, the binary arithmetic, and the diagram
  backend costs hold on the other two platforms is unknown. Expected to graduate
  once the Windows work lands.
- **GPUI's own memory floor, attacked directly.** A bare GPUI window measured
  46.1 MB. Inside that, `load_system_fonts` followed by a deep clone of the font
  database is a plausible target, but it is upstream code and the fix shape is
  not yet clear. Blocked on knowing whether the floor is the same on real
  hardware.
- **The renderer cache eviction policy.** Bounded at 512 entries with
  clear-on-full, carrying a `ponytail:` comment naming LRU as the upgrade path.
  Whether it matters has not been measured.

## Out of scope

- **Selecting image formats out of the `image` crate.** `exr`, `image_webp`,
  `zune_jpeg` and `tiff` total roughly 1.7 MiB. `image` is reached only through
  `gpui`, whose feature list is fixed in zed's own workspace manifest, and cargo
  feature unification means a local `default-features = false` cannot subtract
  from it. This needs an upstream change to gpui, not a change here. Also, the
  formats are not dead: `gpui`'s `Img::extensions()` lists them, so a document
  embedding one would break.
