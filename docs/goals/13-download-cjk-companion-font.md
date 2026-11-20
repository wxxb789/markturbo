# Goal 13 — Download and apply a Chinese companion font on request

## Objective

Chinese text pairs with the embedded Martian faces through an open-source
companion font that the user downloads and applies from Settings with one
click. The font is never bundled in the executable, and nothing leaves the
device without that click.

## Done when

1. Settings shows a Chinese-font entry. Before any request, it states the
   destination host, the size, the licence, and that no document content is
   sent. Its states are Not installed, Downloading (progress and Cancel),
   Installed (Remove) and Failed (reason and retry), in English and Simplified
   Chinese.
2. Clicking the action downloads Source Han Sans CN VF 2.005 (SIL OFL 1.1)
   from the pinned `raw.githubusercontent.com` URL at the `2.005R` tag:
   - it is capped at 15,636,088 bytes and verified against the pinned SHA-256;
   - it is renamed atomically into `<data>/fonts/`;
   - a network, verification or disk failure, or a cancel, leaves no file.
3. At launch, a present file is verified and registered off the first frame,
   and makes no request. A file that fails verification is removed and
   treated as absent.
4. `Source Han Sans CN VF` leads the CJK position of `FONT_UI`, `FONT_MODEL`,
   `FONT_MEDIUM` and `FONT_SOURCE` on every platform. The Windows `FONT_INPUT`
   is unchanged.
5. `cargo test --profile ci -p mt-core --features model-transport` and
   `cargo test --profile ci -p mt-app` pass, and so does the per-PR CI.
   Download tests run against a loopback server only and cover:
   - correct bytes;
   - wrong bytes;
   - oversize;
   - truncated/404;
   - an existing file kept on failure;
   - cancel.

   The headless Settings test proves zero requests before the click.
6. A manual Windows release-build run downloads the real file through the
   Settings button (size and SHA-256 match, no leftovers). After relaunch,
   zh-CN text renders in the companion face.

## Failure control

- Any request without the click: at startup, in the background, or as an
  automatic retry.
- A partial or unverified file left in `<data>/fonts/`, or bytes registered
  before verification.
- A test reaching a non-loopback host.
- Startup work on the first-frame path for this feature.

## Out of scope

- Bundling any font, and installing fonts system-wide.
- Per-script weight compensation for CJK.
- Making Windows inputs use registered fonts (needs an nsh upstream change).
- Fonts other than Simplified Chinese.

## Evidence plan

- Local: the two test commands above, plus `mt.py check fast` and clippy.
- CI: the per-PR pipeline on the PR head.
- Manual: the end-to-end run in clause 6, recorded as size, SHA-256 and a
  with/without screenshot pair under the git-ignored `.scratch/fonts/e2e/`.

## Mergeable alone

Yes, after Goal 12 has landed on `main`, which it builds on. Without the file,
behaviour is identical to Goal 12.
