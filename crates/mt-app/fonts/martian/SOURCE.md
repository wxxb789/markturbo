# Martian Grotesk / Martian Mono, vendored

The Afterglow UI is set in Martian Grotesk (UI, preview, model voice) and
Martian Mono 400 (source mode). Both families are by Evil Martians and are
distributed under the SIL Open Font License 1.1; the licence texts shipped
beside the fonts are `OFL-MartianGrotesk.txt` and `OFL-MartianMono.txt`, taken
verbatim from the upstream repositories' `OFL.txt`.

## What is vendored

| File | Family name in the font | Source |
|---|---|---|
| `MartianGrotesk-StdLt.ttf` | `Martian Grotesk Std Lt` (weight 300) | `martian-grotesk-1.0.0-ttf.zip` |
| `MartianGrotesk-StdRg.ttf` | `Martian Grotesk Std Rg` (weight 400) | `martian-grotesk-1.0.0-ttf.zip` |
| `MartianGrotesk-StdMd.ttf` | `Martian Grotesk Std Md` (weight 500) | `martian-grotesk-1.0.0-ttf.zip` |
| `MartianMono-CnRg.ttf` | `Martian Mono Cn Rg` (weight 400, condensed width) | `martian-mono-1.1.0-ttf.zip` |

## Where they came from

* Martian Grotesk v1.0.0 (release tag `v1.0.0`):
  https://github.com/evilmartians/grotesk/releases/download/v1.0.0/martian-grotesk-1.0.0-ttf.zip
  sha256 `31f4c8f55b89bf6d7b3c934c083ef78a579ffd4a0d9003268f92563fee727730`
* Martian Mono v1.1.0 (release tag `v1.1.0`):
  https://github.com/evilmartians/mono/releases/download/v1.1.0/martian-mono-1.1.0-ttf.zip
  sha256 `483aa2d201a6c44610c0bdcf65eaf425b467d61e9ab606213eacef3e216c2a12`
* Licences: https://raw.githubusercontent.com/evilmartians/grotesk/main/OFL.txt and
  https://raw.githubusercontent.com/evilmartians/mono/main/OFL.txt — both are
  "SIL Open Font License Version 1.1" with reserved font names
  "Martian Grotesk" and "Martian Mono".

The fonts are unmodified static instances straight from those archives.

## Why these instances

The Afterglow prototype (`Q:/repos/markturbo/.scratch/ui-redesign/r2/afterglow/`,
not part of this repository) embedded woff2 subsets whose internal names were
`Martian Grotesk Std Lt` / `Std Rg` / `Std Md` and `Martian Mono Cn Rg`, i.e. it
was drawn with exactly these four instances at 300 / 400 / 500 and, for the
mono, the condensed (0.6 em) advance rather than the standard 0.7 em one. The
native port uses the same instances so the two look alike.

Registering these is `crates/mt-app/src/ui/fonts.rs`; the family names above are
what `ui::tokens` passes to `font_family` strings. See `ui::tokens` for the
per-platform CJK fallbacks in those family lists.
