# vendor/ratex-parser

`ratex-parser` 0.1.14 from crates.io, with one 25-line patch applied.

## Why this exists

`\begin{alignat}{N}` allocates `N * 2` `AlignSpec` values with no upper bound.
`AlignSpec` is 64 bytes, so an unclamped `N` is an allocation request the
allocator cannot satisfy — and an allocation failure **aborts**, which
`catch_unwind` cannot contain. `RendererRegistry::render` wraps every renderer in
`catch_unwind` precisely so a third-party bug becomes a diagnostic rather than a
lost session; this is the one class of failure that defeats it.

Measured against the unpatched crate: a 45-byte document requests
68,719,476,736 bytes and the process dies with
`memory allocation of 68719476736 bytes failed`.

## Why the patch is here and not in markturbo

A guard in `MathRenderer::render` was tried first and cannot work. It scans the
source text, while the parser tokenises and macro-expands before reading the
argument, so the two never agree. Every one of these defeats a source-text
prefilter and is rejected by the patch:

```latex
\begin {alignat}{1000000000}                    % space before the brace
\begin%c
{alignat}{1000000000}                           % comment between them
\def\N{1000000000}\begin{alignat}{\N}           % macro-supplied count
\def\EE{alignat}\begin{\EE}{300000000}          % macro-supplied env name
```

The prefilter also false-positived on ordinary LaTeX — `\begin{cases}{-1} & x<0`
and `\begin{pmatrix}{1000} & 0` both open with a braced group that is not a
column count.

## The patch

One hunk in `src/environments.rs`, at the `alignat`/`alignedat` column count.
It clamps to 256 columns and returns a `ParseError` above that, rather than
silently ignoring an argument it will not honour. Empty and unparseable
arguments fall through to the existing `is_aligned` path, so `\begin{aligned}`
and `\begin{array}{p{2cm}c}` are untouched.

`diff -u` against the registry copy is 32 lines including context.

## Verification

- The crate's own test suite passes unchanged: 158 + 3 + 1 doctest, 0 failed.
- All eight bomb shapes are rejected; `alignat{2}`, `alignat{256}`, `cases`,
  `pmatrix` and `array` still parse.

## When to delete this

When a published `ratex-parser` release carries the clamp. Then drop the
`[patch.crates-io]` stanza from the workspace `Cargo.toml`, delete this
directory, and bump the pin.

Upstream: https://github.com/erweixin/RaTeX

## Provenance

Everything except `src/environments.rs` is byte-identical to the crates.io
release. `Cargo.toml.orig` and `.cargo_vcs_info.json` are the registry's own
files and record the upstream commit.
