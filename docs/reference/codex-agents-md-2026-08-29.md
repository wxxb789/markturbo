# Codex `AGENTS.md` profile (2026-08-29)

This is the frozen `codex-agents-md-2026-08-29` profile used by Effective
Agent Context. It is based on inspected OpenAI Codex revision
`b8c86376a258e55efc8e5ecfbabc21c16c07d814` (2026-08-29T20:42:12Z), retrieved
2026-10-02. Runtime resolution reads the checked-in JSON, never live
documentation.

## Authority and digest transcript

The user-facing contract is
<https://developers.openai.com/codex/agent-configuration/agents-md>. The
implementation evidence is the raw response at each immutable URL below. The
digests are SHA-256 of raw response bytes, not converted Markdown:

| source | immutable URL | SHA-256 |
| --- | --- | --- |
| `codex-rs/core/src/agents_md.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/core/src/agents_md.rs) | `8bbaf068c099fdeeaf4fe49076d398da7671f20c9a962fde5c4eb7653008fed4` |
| `codex-rs/codex-home/src/instructions/mod.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/codex-home/src/instructions/mod.rs) | `fad349ac9be95dfcbf5401d814f35a8cb118ab6cdfe3cf48ae89fda8b39e4614` |
| `codex-rs/config/src/project_root_markers.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/config/src/project_root_markers.rs) | `21d3a86eea7a34f39b1ebc82037d78dd9c1c46fef0c4c74de584bae7cab17f53` |
| `codex-rs/core/src/config/mod.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/core/src/config/mod.rs) | `7e5f8df04304e70451abef58d220ce87417092bbdb8f8f040b5223fa0c6e2bd3` |
| `codex-rs/config/src/config_toml.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/config/src/config_toml.rs) | `125cf3a29c57890e78e06c0a0ee5fcaf853bd370dbdbd460572e22bbac47fec6` |
| `codex-rs/core/src/agents_md_tests.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/core/src/agents_md_tests.rs) | `f3c2c04087eed398afef2b02ab6955166586b63ebf6788d558534da617e1409a` |
| `codex-rs/ext/skills/src/fragments.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/ext/skills/src/fragments.rs) | `4e61b21994402a08554ae02352a3eb5c99f13f7a7f974ec656a22a3b5eb05a42` |
| `codex-rs/utils/home-dir/src/lib.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/utils/home-dir/src/lib.rs) | `1338259489ade54b01caa1468b6d25fc5a183657fa94cca0add8b91865635ef5` |
| `codex-rs/file-system/src/find_up.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/file-system/src/find_up.rs) | `5e24e7a4d593b47fafbada89a84fc7917c8e3b26a9acd30b93341b2e70a7ff19` |
| `codex-rs/file-system/src/lib.rs` | [raw source](https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/codex-rs/file-system/src/lib.rs) | `8755f5ba4d6599a5c5757ffa9acb2eac1c042cc42175f9b3ef87df72f9afae5b` |

The JSON is authoritative for machine use. Its `rules` array gives the exact
source path and line range for every frozen rule.

## Frozen behavior

Codex loads global instructions first, then one selected project file per
directory from project root to execution cwd. Global selection tries
`AGENTS.override.md` then `AGENTS.md`, choosing the first readable non-empty
trimmed text. Project selection checks override, `AGENTS.md`, and configured
fallback names; the first existing regular file wins before content is checked,
so an empty override does not fall through.

The default root marker is `.git`. No marker, or an empty marker set, makes
the cwd the only project directory. The configured fallback default is empty;
`AGENTS.md` is a built-in candidate. Fallback names are trimmed, empty names
are dropped, and exact duplicates are removed. The default raw project budget
is 32768 bytes: global text and separators are excluded, raw bytes are
truncated before lossy UTF-8 conversion, and empty text consumes no budget.

`CODEX_HOME` comes from the environment when it is non-empty, without
whitespace trimming; Codex requires that path to exist as a directory and
canonicalizes it. Without the environment variable, Codex uses
`<platform home>/.codex`. Project-root marker lookup searches ancestors and
selects the nearest configured marker. When marker configuration is unset, the
default is `[.git]`; when no marker is found or the configured list is
explicitly empty, the cwd is the only project directory. File reads follow
symlinks by default. Global instructions and separators do not consume the
project byte budget; project reads do.

Reads follow symlinks but preserve lexical discovery paths and cwd. Explicit
untrusted project selection retains global instructions and excludes project
content. AGENTS text is opaque: frontmatter, imports, and scope-like syntax do
not change applicability or execute imports. Skills remain available on demand
and are not automatically concatenated. Unsupported or malformed constructs
are MarkTurbo product diagnostics, not upstream discovery rules.

This profile does not claim content or physical-file deduplication as Codex
rules. MarkTurbo may deduplicate a display or Review projection only if it
retains actual occurrence order, provenance, and budget effects. It also does
not claim layered configuration, sandbox, multiple environments, plugin
fidelity, or resolution semantics for other harnesses.
