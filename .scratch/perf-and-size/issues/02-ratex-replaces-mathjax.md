# 02 — Replace MathJax with RaTeX, lazily, embedding no fonts

Type: task
Status: resolved

## Question

Replace MathJax with a pure-Rust math backend, subject to four constraints the
user set:

1. **Lazy** — math initializes only when a document contains a math block.
2. **Embed no fonts** — project-wide policy. A required font the user does not
   have is something markturbo asks them to install, not something it ships
   inside the executable.
3. **Fix or avoid three defects** in `ratex-unicode-font`: `eprintln!` past the
   `log` crate, hardcoded platform font paths, and +46 MB resident on the first
   CJK glyph that is never released.
4. Repository conventions: one ticket one commit, measured before and after,
   `mt-doc` stays GPUI-free, `panic = "unwind"` stays.

## Answer

All four met. The configuration is RaTeX **without `ratex-svg`**, with the SVG
emitted here instead, and with `ratex-parser` vendored under a
`[patch.crates-io]` carrying one 25-line clamp.

### Why not `ratex-svg`

It is the obvious dependency. The only way to get `<path>` rather than `<text>`
out of it is its `standalone` feature, and `standalone` reaches
`ratex-unicode-font` through **two independent edges** — a direct optional
dependency, and a non-optional dependency of `ratex-font-loader`. No feature
combination avoids it; all four the manifest allows were built and measured.

`ratex-unicode-font` is where all three defects live. Emitting the SVG here is
about 250 lines and deletes all three at the source rather than mitigating them.

Verified:

```
cargo tree -e normal | grep -ci "ratex-unicode-font|ratex-svg|ratex-font-loader|system-fonts"
0
```

### The three defects, measured

| Defect | Before | After |
|---|---|---|
| stderr past `log` | 2 lines per process, on first CJK | 0 — the crate is not in the graph |
| hardcoded font paths | 5 distro-specific paths, Debian/Ubuntu-only on Linux | none in the binary; `font_dir_candidates` searches conventional per-user and system directories |
| CJK memory | 4.5 MB → **52.0 MB**, retained for the process | 4.34 MB → **4.55 MB** after 1000 CJK renders |

The 46 MB was `ab_glyph`'s parsed representation of `NotoSansSC-VF.ttf`
(17,773,244 bytes on disk, so about 2.6x). It is gone because CJK now emits
`<text>` and resolves against the font database gpui already populates from the
system, rather than RaTeX loading a font of its own.

`RATEX_UNICODE_FONT` was measured as a partial workaround before this
configuration was found — it controls `CjkRegular` but not `CjkFallback`, whose
loader is documented as "always discovers a system font, ignoring
`RATEX_UNICODE_FONT`". It reduced the cost to 36.2 MB rather than removing it.
Recorded so the weaker option is not rediscovered.

### The abort, and why the guard had to move into the parser

`\begin{alignat}{N}` allocates `N * 2` `AlignSpec` values with no bound.
`AlignSpec` is 64 bytes, so an unclamped `N` is an allocation failure — and an
allocation failure **aborts**, which `catch_unwind` cannot contain. A 45-byte
document is enough:

```
memory allocation of 68719476736 bytes failed
```

A guard in `MathRenderer::render` was written first and **does not work**. It
scans the source text while the parser tokenises and macro-expands before
reading the argument, so the two never agree. Every one of these defeats it:

```latex
\begin {alignat}{1000000000}              % space before the brace
\begin%c
{alignat}{1000000000}                     % comment between them
\def\N{1000000000}\begin{alignat}{\N}     % macro-supplied count
\def\EE{alignat}\begin{\EE}{300000000}    % macro-supplied environment name
```

It also false-positives on ordinary LaTeX — `\begin{cases}{-1} & x<0` and
`\begin{pmatrix}{1000} & 0` both open with a braced group that is not a column
count.

The clamp is therefore in `vendor/ratex-parser/src/environments.rs`, 25 lines,
after macro expansion. Measured against the patched crate, all eight bomb shapes
are rejected in about 1 ms each and all four legitimate forms still render.
The vendored crate passes its own suite unchanged: 158 + 3 + 1 doctest, 0 failed.

### Lazy

`MathRenderer` is a ZST and there is no warm-up to spawn. `main.rs:98`'s
`renderer::warm_up()` is deleted along with the function and its test.

`availability()` reaches `font_dir()`, which **stats only** — it runs from
`render_status_bar` on every frame, so reading a byte there would put half a
megabyte of font on the first frame of a workspace that may never show a
formula. The faces are read on first render, behind a second `OnceLock`.

Measured by `first_formula_costs_little_more_than_the_rest`, which replaces the
old `cold_math_costs_far_more_than_warm_math` harness — its premise was a cold
JS engine and it was `#[ignore]`d, so nothing would have caught it going stale:

```
first 11.8ms  subsequent 1.7ms
loading the faces costs 10.1ms once
```

Against MathJax's 792 ms. That is the number justifying the absence of a
warm-up: 10 ms once is not worth deferring, and if it ever grows back to
hundreds of milliseconds the harness will say so.

### Fonts

Nineteen KaTeX faces, `fonts/katex/` in the repository, staged next to the
executable by `package-release.sh`, searched for by
`font_dir_candidates()` in this order: `MT_MATH_FONT_DIR`, beside the
executable, the repository's own `fonts/katex` (so `cargo run` and `cargo test`
work from a source tree), then the conventional per-user and system font
directories.

None of their bytes are in the binary. When no candidate holds all nineteen,
`availability()` returns `Missing` with an install hint and every formula
becomes a diagnostic — the shape `PlantUmlRenderer` has always used.

KaTeX is MIT; the faces are built from Metafont sources in KaTeX's own tree and
carry no separate licence. `fonts/katex/LICENSE.md` records this.

### What this also fixes

MathJax permanently failed an entire class of control sequences, reproduced 9/9
with zero recoveries and the error `MathJax retry -- an asynchronous action is
required`: `\mathbb`, `\mathcal`, `\mathfrak`, `\mathsf`, `\mathtt`, `\ell`,
`\Re`, `\Im`. Blackboard-bold rendered as a diagnostic in production. RaTeX
renders all of them; all eight alphabet families produce distinct glyph
outlines, checked by comparing the emitted path data rather than by eye.

### Measured

| | MathJax | RaTeX |
|---|---|---|
| Resident, empty workspace | 87.5 MB paid unconditionally at startup | 0 until the first formula |
| Resident, after 1000 formulas | — | 4.55 MB |
| First render | 792 ms | 1.36–1.87 ms |
| Per formula | ~146 ms | 0.50–0.92 ms |
| KaTeX 90-formula corpus | 87/90 | 89/90 |

The one corpus formula RaTeX rejects is `\label`, which KaTeX itself does not
support.

Rendering was checked by looking at the rasterized output, not only by counting
inked pixels: the quadratic formula, a stretchy `\left(...\right)` over a nested
fraction, a 5x5 `pmatrix`, `\mathbb{R} \subset \mathbb{C}`, `\text{中文}` under
a populated font database, and `\textcolor{red}{x}+y` on a dark background —
where the `x` is red and the `+y` follows the theme.

### Guarded by

- `constructing_the_registry_does_not_load_a_font` — replaces the warm-up test,
  and pins the opposite property.
- `column_count_bombs_are_rejected_rather_than_aborting` — all six shapes,
  including the four a source-text guard misses, plus four legitimate forms.
- `an_oversized_formula_is_capped` — 16 KB cap. The largest math block in this
  repository is 171 bytes; a 200,000-cell matrix costs 424 ms to parse, 794 ms
  to lay out, and produces a ~289 MB string.
- `glyph_colour_defaults_to_currentcolor_and_textcolor_survives` — one SVG has
  to serve twelve themes and the OS light/dark switch.
- `the_diagram_backends_need_nothing_installed` — replaces
  `three_of_four_renderers_are_always_available`. LaTeX is no longer `Builtin`,
  deliberately: MathJax carried its glyphs in a compiled-in JS bundle and this
  application ships them beside the binary instead. What the test still pins is
  that math is never *silently* unavailable.
