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

## Not yet specified

- **macOS and Linux.** Every number in this effort is from one Windows machine
  with no GPU. Whether the memory floor, the binary arithmetic, and the diagram
  backend costs hold on the other two platforms is unknown. Expected to graduate
  once the Windows work lands.
- **GPUI's own memory floor, attacked directly.** A bare GPUI window measured
  46.1 MB. Inside that, `load_system_fonts` followed by a deep clone of the font
  database is a plausible target, but it is upstream code and the fix shape is
  not yet clear. Blocked on knowing whether the floor is the same on real
  hardware.
- **Incremental skill discovery.** The global scan is correctly backgrounded but
  has no incrementality. The prior pass established that a root-directory mtime
  check is not sufficient, because a skill edited inside an unchanged directory
  would be missed. What *is* sufficient is not yet worked out.
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
