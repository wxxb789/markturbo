# Goal 12 — Pair each platform's built-in fallback fonts with Martian

## Objective

After the embedded Martian faces, every font list falls back to faces the
platform ships that sit best with Martian, and Windows text inputs never fall
to a serif. Martian stays embedded; no font is added to the executable.

## Done when

1. `crates/mt-app/src/ui/tokens.rs` gives `FONT_UI`, `FONT_MODEL`,
   `FONT_MEDIUM` and `FONT_SOURCE` per-platform fallbacks after the two Martian
   names, CJK face first:
   - Windows: DengXian → Microsoft YaHei UI → Bahnschrift → Segoe UI Variable
     (as the optical-size families DirectWrite resolves: `Segoe UI Variable
     Text`, and `Segoe UI Variable Display` for the model voice); code:
     Cascadia Mono → Consolas.
   - macOS: PingFang SC → Hiragino Sans GB → `system-ui`; code: `ui-monospace`
     → Menlo.
   - Linux: Noto Sans CJK SC → Source Han Sans SC → WenQuanYi Micro Hei → Inter
     → Adwaita Sans → Noto Sans; code: Noto Sans Mono CJK SC → Noto Sans CJK SC
     (proportional CJK when the mono CJK face is absent) → Adwaita Mono →
     DejaVu Sans Mono.

   The Native Preview body uses `FONT_UI`, not a list of its own.
2. The four text inputs use `FONT_INPUT`. On Windows its first family is a
   face Windows ships (Bahnschrift), because nsh 0.5.1's Windows input
   resolves only the first family, from system fonts only. Elsewhere it equals
   `FONT_UI`.
3. `cargo test --profile ci -p mt-app` passes on Windows, and so does the
   per-PR CI. This includes a test of each platform's exact ordered fallback
   suffix per role and the Windows CJK fallback probe
   `document::tests::the_cjk_fallback_prefers_dengxian`. The per-PR macOS job
   builds but does not run tests, so the macOS token tests are evidenced by a
   Nightly run dispatched on the PR head, whose macOS heavy-tests job runs
   them.
4. A release-build visual capture of `settings` and `review-result-questions`
   (en-us and zh-cn, light and dark) shows Settings input placeholders in a
   sans face, not a serif, and zh-CN text set in the CJK fallback.

## Failure control

- Any input placeholder still in a serif in the Windows capture.
- A Martian face no longer first in a list, or a CJK face that is not directly
  after the two Martian names.
- A token test passing on a platform where the list it asserts is not the one
  compiled.

## Out of scope

- Downloading or bundling any additional font (Goal 13).
- Per-script weight compensation for CJK.
- An upstream nsh change that lets Windows inputs use registered fonts.
- Accessibility-only work.

## Evidence plan

- Local: `cargo test --profile ci -p mt-app`, Windows host.
- CI: the per-PR pipeline on the PR head (Linux tests, the macOS build,
  Windows release tests), plus a Nightly run on the PR head for the macOS
  tests.
- Native: `mt.py visual -- --exe <release mt-app.exe>
  --out .scratch/goal-12-visual --states settings,review-result-questions
  --languages en-us,zh-cn --viewports default` on the release build. Capture
  receipts stay under the git-ignored `.scratch/`.

## Mergeable alone

Yes. The change is limited to font-family strings and the input family. It
needs no sibling change, and Goal 13 builds on it.
