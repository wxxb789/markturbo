# Dependency review — 2026-09-22

Reviewed workspace Cargo.toml, Cargo.lock, scripts/pyproject.toml, scripts/uv.lock,
and upstream registry metadata. Versions below are observations on this date,
not a promise that an upstream update is compatible. This is not a complete
transitive vulnerability audit. Validation results belong in the PR.

## Compatible updates in this change

| Dependency | Previous lock | Proposed lock | Reason and validation focus |
| --- | --- | --- | --- |
| `encoding_rs` | 0.8.35 | 0.8.41 | Legacy-decoder boundary and panic-safety fixes; review encoding/file round-trip tests. Adds SIMD support dependencies and raises MSRV to 1.88. |
| `reqwest` | 0.13.4 | 0.13.5 | Proxy credential-selection and timeout handling fixes; preserve endpoint, consent and transport tests. Adds a separate base64 0.23 dependency. |
| `rustls` | 0.23.43 | 0.23.45 | Fixes TLS 1.3 handshake encryption-level validation; keep explicit ring provider initialization. |
| `toml` | 1.1.4+spec-1.1.0 | 1.1.6+spec-1.1.0 | Compatible patch update; validate settings parsing/round trips. |
| `ruamel.yaml` | 0.18.17 | 0.19.1 | Update development YAML parsing; run workflow/tooling tests against the resolved version. |

The four Rust manifest ranges already allow these versions. Cargo generated the
lock update; GPUI/component Git identities, feature selections and the RaTeX
patch remain unchanged. Cargo also re-resolved some Windows dependency edges;
cross-platform CI must validate the resulting graph, not just the direct crates.
Do not interpret headless document tests as transport or native validation.

Python resolution now uses public PyPI rather than the Microsoft feed URLs
in the previous lock. GitHub workflows use `uv run --locked`, so CI cannot
silently rewrite the lock while validating a commit. The 0.19.1 YAML wheel is
pure Python and no longer needs the previous ruamel.yaml.clib lock entry.

Upstream references:

- [encoding_rs release notes](https://github.com/hsivonen/encoding_rs/blob/master/README.md#release-notes)
- [reqwest 0.13.5](https://github.com/seanmonstar/reqwest/releases/tag/v0.13.5)
- [rustls 0.23.45](https://github.com/rustls/rustls/releases/tag/v/0.23.45)
- [toml 1.1.6](https://docs.rs/crate/toml/1.1.6+spec-1.1.0)
- [ruamel.yaml 0.19.1](https://pypi.org/project/ruamel.yaml/0.19.1/)

## Separate migrations, not blind updates

| Dependency | Current | Available/observed | Decision |
| --- | --- | --- | --- |
| `serde_yaml` | 0.9.34+deprecated | Upstream deprecated | Prioritize a maintained parser evaluation separately. Test frontmatter, duplicate keys, aliases, malformed YAML, diagnostics and source preservation before choosing a replacement. A newer crate name is not sufficient evidence. |
| `dirs` | 6.0.0 | 7.0.0 | Major migration. Validate Windows/macOS/Linux config/data paths; a changed directory must not make settings or recovery appear lost. |
| Workspace `sha2` | 0.10.9 | 0.11.0, already transitive | Breaking 0.x version step. Evaluate API migration and exact digest fixtures separately; transitive 0.10 users may still prevent deduplication. |
| Workspace `windows` | 0.61.3 | 0.62.2, already transitive | Coordinate native API and handle-type interoperability; requires Windows compilation and affected native smoke. |
| GPUI + component/base/assets/wry | Kit family 0.6.6 + gpui-pre 0.3.6 | Coordinated registry migration | Migrated in the separate Kit change; native acceptance remains pending. Do not chase Zed HEAD. |
| RaTeX family | 0.1.14 | 0.1.14 | Keep the vendored parser allocation clamp until a verified upstream release includes it. |

Registry checks also found no newer stable release for the directly used
`genai` 0.6.5, `mermaid-svg` 0.7.0, `d2-little` 0.7.1-1, `markdown` 1.0.0,
`lb-wry` 0.53.3, `notify` 8.2.0, `notify-debouncer-full` 0.7.0,
Pillow 12.3.0 or pywinauto 0.6.9. Do not replace these just to increase a
version number.

Reproduce version discovery using the primary APIs
`https://crates.io/api/v1/crates/<crate>` and
`https://pypi.org/pypi/<package>/json`; filter prereleases and yanked releases,
then compare with the actual lockfile, not only the manifest range.

## GPUI Kit migration

Before migration the workspace used Zed GPUI at `8ee36b682cf1971e51032cbd932dd16def575364`
and the component family at `14ba7869c9adb3c0684b604f1d1394858a98de2b`.
Upstream has rebranded to GPUI Kit and moved the family to crates.io snapshots.
The observed non-yanked versions are **Kit/component/base/assets/wry 0.6.6** and
**gpui-pre/gpui-pre-platform 0.3.6**. The published 0.6.6 manifests pin GPUI to
`=0.3.6`. Kit/component/base/assets 0.6.5 are yanked; do not choose them from an
older GitHub manifest or search snippet. Registry package checksums were verified
before inspecting the 0.6.6 source/manifests.

Concrete benefits for MarkTurbo, established in upstream source/history:

- Kit exposes headless input, click, scroll, focus, layout and callback helpers
  through `test-support`, `TestWindowExt` and `TestAppContextExt`. This can reduce
  future custom UI-test plumbing. It does not remove Windows integration tests.
- Markdown TextView fixes include render-loop prevention (#2893), remeasuring
  equal-block-count replacements (#2946), cheaper per-frame elements (#3090),
  and retaining shaped paragraph text/highlights across frames (#3115).
- Editor fixes include CRLF cursor/shortcut boundaries (#2986), fewer redundant
  scroll/paint notifications (#2988), revealing search matches (#2955/#3013),
  and avoiding blinking on unfocused inputs (#3140).
- Exact GPUI snapshot binding addresses the previous risk of a compatible-looking
  version range selecting a snapshot that the component release cannot compile
  with (#3163). Registry dependencies also avoid fetching the whole Zed Git tree.

These are upstream changes, not measured MarkTurbo performance gains. Zed editor
features are not automatically GPUI features. Likewise, commits after the chosen
published snapshot must not be counted as benefits already in that package.

Implemented migration boundary:

- Application imports, startup and tests use the `gpui_kit` facade. Kit resolves
  component/base/assets 0.6.6 and GPUI/platform 0.3.6 from crates.io; the old Zed
  and component Git sources are removed. `gpui-wry` is pinned to 0.6.6 and
  remains Windows/macOS-only with lb-wry 0.53.3.
- Kit does not forward `stacker`, so the direct `gpui` alias for exactly
  `gpui-pre =0.3.6` enables it on the same package. The original grammar subset,
  profiler wiring and optimized dev package overrides are preserved.
- Kit `test-support` stays in dev-dependencies. Existing context fixtures and
  behavioral tests remain; the welcome regression uses `TestWindowExt` to
  click New, check welcome dismissal and a single buffer, then type through
  the focused input. No second harness or desktop session is required.
- The migration is separately reviewable from development-process changes.
  All three platform compile/test gates are required before merge. Focused
  Windows CJK/IME, selection/undo, clipboard, save-dialog and WebView-focus
  acceptance remains pending against the upgraded artifact. Headless passing
  does not resolve historical Goal 03 native evidence.

See `docs/development.md` for the targeted command and testing boundary. No
application performance or production binary-size improvement is claimed.

Sources: [published Kit 0.6.6](https://docs.rs/crate/gpui-kit/0.6.6),
[registry metadata](https://crates.io/api/v1/crates/gpui-kit),
[published Kit source](https://docs.rs/crate/gpui-kit/0.6.6/source/), and the
[upstream changes](https://github.com/longbridge/gpui-kit/compare/14ba7869c9adb3c0684b604f1d1394858a98de2b...9765ae2c9a5eccfa13891248a445991e6f6a09d8).
Headless snapshots inspect state/layout, not GPU-rendered pixels. The separate
upstream rendering test explicitly requires real Metal hardware.

## Follow-up priorities

1. Review the `serde_yaml` replacement as its own small domain change.
2. Measure PR durations with the new profile before changing CI platforms,
   splitting more jobs or introducing path-selection machinery. The Windows
   target and existing compatibility coverage remain useful.
3. Replace fragile source-text assertions only when touching their behavior;
   preserve meaningful privacy and lifecycle invariants. Do not start a broad
   test rewrite as a prerequisite to the next product goal.
4. Review dependencies in small batches. Keep native stack upgrades separate
   from ordinary tooling/transport patches so their acceptance cost is explicit.
