# 02 — Replace MathJax with ReX, and make math rendering lazy

Type: task
Status: claimed
Blocked by: none

## Question

Two things the user decided together, because they touch the same code path:

1. Math rendering must not initialize at startup. It initializes when a document
   actually contains a math block, and a settings toggle can disable it outright.
2. MathJax is replaced by ReX, a pure-Rust TeX typesetter, which removes the Boa
   JavaScript engine from the dependency graph entirely.

What does the replacement actually cost, what does it break, and what does the
resulting `MathRenderer` look like?

## Why both at once

They are the same edit. `renderer::warm_up()` exists only to pay MathJax's
engine start-up early; with ReX there is no engine to warm, so the lazy question
and the backend question resolve in the same place. Doing them separately would
mean writing a warm-up-gating mechanism and then deleting it.

## What is established

Measured in a standalone probe at `Q:/tmp/tex-probe/rexsvg`, reproduced
independently by two verification agents:

| | MathJax + Boa | ReX |
|---|---|---|
| Resident memory | 87.5 MB | 4.5 MB |
| First render | 792 ms | 0.10 ms |
| Per formula | ~146 ms | 0.29-0.63 ms |
| Probe binary | 12,921,344 B | 1,411,584 B |
| Output | `<defs><path>` + `<use>` | bare `<path>`, zero `<text>`/`<use>`/`<defs>` |
| resvg, empty fontdb | — | 13/13 rasterize with ink |

Boa leaves the graph completely. `cargo tree -p mt-app -i boa_engine` shows one
parent, `mathjax-svg-rs`. `boa_parser`, `boa_ast`, `boa_gc`, `boa_interner`,
`boa_string`, `boa_macros` hang off `boa_engine` alone, and so does `regress`
(0.10.5). Together roughly 5.2 MiB of `.text`, plus the 1.6 MB zstd-compressed
MathJax bundle in `.rdata`.

`icu_normalizer` and `icu_properties` do **not** leave: they have a second
parent through `idna` -> `url` -> `gpui`.

### The live defect this also fixes

`mathjax-svg-rs::render_tex` permanently fails an entire class of control
sequences, reproduced 9/9 with no recoveries, error
`MathJax retry -- an asynchronous action is required`:

```
FAIL: \mathbb  \mathcal  \mathfrak  \mathsf  \mathtt  \ell  \Re  \Im
OK:   \aleph  \nabla  \emptyset  \hbar  \mathbf  \mathrm  \infty  \partial
```

The pattern is the alternate-alphabet constructs. `renderer.rs:246` calls that
exact API, so blackboard-bold currently renders as a diagnostic in production.
ReX renders all of them.

An earlier draft of this attributed the failure to `\aleph`; that was wrong —
`\aleph` renders fine. Verify per token before citing one as broken.

### Coverage, the real cost

Against a 90-formula corpus: ReX 69/90 raw (76.7%), MathJax 87/90. With
`\newcommand` shims (recovers 8) plus a textual environment rewrite (recovers
7/7), ReX reaches 84/90 (93.3%).

Residual gaps: `\boxed`, `\atop`, `\choose`, `\bmod`, `\ce`. Grepping every
tracked `.md` in this repository for those constructs returns zero occurrences.

### The supply-chain cost, stated plainly

ReX is **not published on crates.io**. It is a git dependency on
`https://github.com/KenyC/ReX` at `aeccdba`, a fork with roughly 25 stars, and
it carries a path sub-crate `deps/unicode-math` that is also unpublished — so
vendoring means vendoring two crates.

Every other dependency in this repository comes from crates.io or from an
upstream this project already tracks (zed, gpui-component). This is a posture
the project has not taken before, and it is the reason this ticket is a decision
rather than a mechanical swap.

The SVG backend is app-owned code: roughly 75 lines implementing `FontBackend`,
`GraphicsBackend` and `Backend`, plus a `ttf_parser::OutlineBuilder`. It is
small, but it is ours to maintain and test — not a footnote.

A math font must also be embedded. `rex-xits.otf` is 683,132 bytes;
`FiraMath_Regular.otf` is 179,840 and is the smallest of the four ReX ships.

## Answer

<!-- filled in on resolution -->
