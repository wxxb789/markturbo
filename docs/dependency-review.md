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
| GPUI + component/base/assets/wry | Git-locked | Both upstream repositories have newer HEADs | Do not chase HEAD in this workflow change. Upgrade the group together for a concrete bug fix; preserve common source selectors, stacker, grammar features and WebView behavior. |
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
