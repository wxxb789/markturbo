# markturbo

A native, local-first desktop workspace for the Markdown artifacts that humans
and AI agents exchange: `README.md`, `AGENTS.md`, `CLAUDE.md`, `SKILL.md`,
prompts, specs, plans, and MDX.

> This is not a Markdown editor with AI features. It is a native workspace for
> Markdown as the interface between humans and AI agents.

Built with Rust, [GPUI](https://github.com/zed-industries/zed), and
[`longbridge/gpui-component`](https://github.com/longbridge/gpui-component).

## What it does

- Open a directory as a workspace; browse it in a native file tree.
- Open Markdown, MDX, and agent artifacts in tabs.
- Read in **Source / Native / Web / Split** views.
- Render Mermaid, D2, PlantUML, and LaTeX from document source.
- Discover Agent Skills across the ecosystem's conventional roots, parse and
  validate `SKILL.md` frontmatter, and inspect them in a dedicated view.
- Translate a selection, a block, or a whole document while preserving code,
  URLs, diagram source, math, and markup.
- Detect external filesystem changes and refuse to overwrite them silently.

Files stay ordinary files. No import step, no proprietary format — Codex, Claude
Code, Cursor, OpenCode, git, and shell tools consume them unchanged.

## Run a release build

```sh
./scripts/package-release.sh     # builds, verifies, and archives into dist/
```

Or build and run directly:

```sh
cargo run --release -- sample    # opens the sample workspace
cargo run --release -- /my/repo  # or any directory
```

The first build is long: GPUI is compiled from source. See
[docs/platforms.md](docs/platforms.md) for per-platform requirements.

## Architecture

See [docs/architecture.md](docs/architecture.md). The short version:

```text
Filesystem
    │
    ▼
mt-doc  ── document engine, no GPUI dependency
    │      source · blocks · outline · frontmatter · diagnostics · doc type
    │
    ├── mt-app::fs         load/save with conflict protection
    ├── mt-app::renderer   block renderer registry (Mermaid, D2, PlantUML, math)
    ├── mt-app::web        WebView compatibility path
    └── mt-app::views      GPUI views
```

`mt-doc` has no GPUI dependency, so the same model can later drive CLI tooling,
MCP tools, or headless rendering without touching the UI.

## Renderers

| Technology | Backend | Availability |
|---|---|---|
| Mermaid | `mermaid-svg` (pure Rust) | Always |
| D2 | `d2-little` (pure Rust, own layout) | Always |
| LaTeX / math | `mathjax-svg-rs` (MathJax on an embedded JS engine) | Always |
| PlantUML | `plantuml` CLI | Requires a local install (Java) |

Three of the four need no external dependency. PlantUML has no usable pure-Rust
implementation today; when the binary is absent, blocks show an install hint
instead of failing. A renderer that errors or panics produces an inline
diagnostic with the original source preserved — never a crash.

Adding a technology is a registration: implement `BlockRenderer`, register it,
and add the fence language to `DiagramKind::from_lang`. No parser or view change.

## Translation

`TranslationService` is a trait in `mt-doc`; providers live in `mt-app`. The
document engine decides *what* is translatable — prose only, never code, URLs,
link targets, frontmatter keys, diagram source, math, or block markup — and the
provider only ever sees prose fragments.

- **Echo** (default, offline): tags each translatable segment, so structure
  preservation is visible without any credentials.
- **Anthropic**: set `ANTHROPIC_API_KEY`. Optional:
  `MARKTURBO_TRANSLATE_MODEL`, `MARKTURBO_TRANSLATE_TO` (default `zh`).

## Security

MDX can contain executable code. The WebView loads content from an opaque
`data:` origin under a CSP that blocks **all** network access at both trust
levels; `Restricted` (the default for every document) also blocks scripts.
Trusting a document is an explicit per-document action and never opens the
network.

## Keyboard

| Key | Action |
|---|---|
| `Ctrl/Cmd+O` | Open folder |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl/Cmd+F` | Find in editor |
| `Ctrl/Cmd+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl/Cmd+Shift+T` | Translate document |
| `Ctrl/Cmd+Shift+L` | Translate selection |
| `Ctrl/Cmd+Shift+B` | Translate block at cursor |

## Tests

```sh
cargo test --release
```

198 tests across 7 suites, over checked-in fixtures in `fixtures/`: Markdown
constructs including CJK and Unicode, valid and invalid examples of each diagram
technology, MDX (Markdown-only, components, invalid, untrusted), skills (valid,
missing metadata, malformed YAML, nested, multiple roots, name collisions),
filesystem round-trips, and ~10K/~100K-line performance documents.

## License

Apache-2.0
