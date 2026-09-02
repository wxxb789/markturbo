# KaTeX fonts

The nineteen faces `MathRenderer` needs, from
[KaTeX](https://github.com/KaTeX/KaTeX) v0.18.4 (released 2026-08-10).

## Licence

MIT, the same licence as KaTeX itself. The complete upstream v0.18.4 text is
checked in as [`LICENSE`](LICENSE) and embedded in the release executable. The
faces are built from the Metafont sources under KaTeX's own `src/fonts/` and
carry no separate licence of their own.

## Distribution

The Windows release embeds the KaTeX faces in `markturbo-windows-x64.exe`, alongside its
bundled sample. The raw executable is the only CD artifact, so the renderer may
not depend on a sidecar `fonts/` directory. These source files remain in the
repository for licensing, reproducible builds, and development validation.

## Updating

Take the `.ttf` files from `katex/fonts/` in a KaTeX release archive. Nineteen
are used; `KaTeX_Caligraphic-Bold.ttf` is present because the set ships as a
unit and a partial copy is harder to verify than a whole one. `MATH_FONT_FILES`
in `crates/mt-app/src/assets.rs` is the authority on which are required.
