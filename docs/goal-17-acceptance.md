# Goal 17 implementation and acceptance record

Contract: [Layout and content polish](goals/17-layout-content-polish.md).

## Local checks

The light ember is `#8D6240`: OKLCH lightness 0.534 and chroma 0.074, with
4.66:1 contrast against the light room. The previous lightness was 0.535.

Headless checks inspect the resolved text styles, not screenshots or widget
source.
The bundled Martian Grotesk Regular has no `tnum` feature in GSUB/GPOS, so
Goal 17 changing counts use bundled Martian Mono at the surrounding size.

The Revision loopback test withholds the response headers, presses Cancel,
and checks that the client disconnects and the prior Review returns. This
stops the local request; it cannot recall bytes the provider already received.

At a 1280 by 900 headless viewport, Settings actions end at the field's
right edge (x=644). The scroll viewport is 734px high and the form is 1055px
high; the usage section fits inside the viewport after scrolling. Checked
row origins include y=108, 124, 156, 316, 392, 424, 540, 564, 632, 680,
760, and 912.

Temporary regressions failed 21 targeted app unit tests, five Settings
integration tests, and two Revision integration tests (each command exited
101). The tests detected the removed states, incorrect layout, CJK scope,
low chroma, missing tabular figures, incorrect routing, and missing request
cancellation. Adding the CJK comparison to the default visual matrix failed
its isolated Python test (exit 1); restoring the opt-in behavior passed it.
Disabling Settings scrolling separately failed the last-section test because
Headless could no longer scroll the viewport (exit 1).
With routing intact, neutralizing error color and removing the status text's
tabular figures failed the two corresponding style assertions (exit 1).

| Check | Exit | Result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --profile ci -p mt-core -p mt-app --all-targets` | 0 | 3 pre-existing warnings. |
| `cargo test --profile ci -p mt-app` | 0 | 294 passed, 0 failed, 0 ignored. |
| `cargo test --profile ci -p mt-core --features model-transport` | 0 | 835 passed, 7 ignored, 0 failed. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 0 | 235 passed in 8 shards. |
| `uv run --locked --project scripts scripts/mt.py check ci` | 0 | 235 tooling tests passed; 1129 workspace tests passed, 7 ignored, 0 failed. |

## Desktop acceptance

Pending on the owner's active, unlocked Windows 11 x64 desktop. No capture or
native pass is claimed.

- Run native acceptance against the final release executable and its expected
  SHA-256, as described in [verification.md](verification.md).
- Run the visual matrix on the same executable. Capture before and after
  Settings, Review, and light-mode cooled model text. Check Settings at a
  900px window height, including the last section after scrolling.
- Capture the same mixed CJK/Latin paragraph in the three optical variants.
  The default remains unchanged; pick a variant from the captures before
  changing the default in a follow-up.

```powershell
uv run --locked --project scripts scripts/mt.py visual -- --goal17-cjk-optics --exe <release-exe> --out .scratch/goal-17-cjk
```
