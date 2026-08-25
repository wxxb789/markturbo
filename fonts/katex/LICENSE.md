# KaTeX fonts

The nineteen faces `MathRenderer` needs, from
[KaTeX](https://github.com/KaTeX/KaTeX) v0.18.4 (released 2026-08-10).

## Licence

MIT, the same licence as KaTeX itself:

> The MIT License (MIT)
>
> Copyright (c) 2013-2020 Khan Academy and other contributors
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.

The full text is at <https://github.com/KaTeX/KaTeX/blob/main/LICENSE>. The
faces are built from the Metafont sources under KaTeX's own `src/fonts/` and
carry no separate licence of their own.

## Why they are here rather than embedded

This application embeds no font it could instead ask the user to install.
`crates/mt-app/src/assets.rs` is the one exception and a different case: gpui's
SVG renderer requests those two by exact path and every diagram label comes out
blank without them.

These twenty files (540 KB) are *shipped beside* the executable rather than
compiled into it. `scripts/package-release.sh` copies this directory into the
release archive, and `renderer.rs::font_dir_candidates` looks next to the
executable first — so a packaged build finds them with the user doing nothing,
and the binary carries none of their bytes.

A build run from the source tree finds them through the same search, because
`MT_MATH_FONT_DIR` can point here and the conventional user font directories are
searched after. When none of them holds all nineteen faces, `availability()`
reports `Missing` with an install hint and every formula becomes a diagnostic
rather than a blank pane.

## Updating

Take the `.ttf` files from `katex/fonts/` in a KaTeX release archive. Nineteen
are used; `KaTeX_Caligraphic-Bold.ttf` is present because the set ships as a
unit and a partial copy is harder to verify than a whole one. `FONT_FILES` in
`crates/mt-app/src/renderer.rs` is the authority on which are required, and the
directory search requires all of them before reporting the directory usable.
