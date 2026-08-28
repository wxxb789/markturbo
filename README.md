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
- Inspect the active document in the always-available Details panel; selecting
  a Harness artifact shows its specialized details there instead.
- Read in **Source / Native / Web / Split·Native / Split·Web** layouts, each
  offered only where it makes sense: a `.rs` file gets Source alone, an `.html`
  gets the WebView.
- Render Mermaid, D2, PlantUML, and LaTeX from document source.
- Discover Agent Skills and agent instruction files across the conventional
  roots of 80+ harnesses, project-local and global, parse and validate
  `SKILL.md` frontmatter, and inspect them in the Harness panel.
- Search the open tabs, the whole project, or every discovered skill and
  instruction file — including the directories outside the open folder.
- Translate a selection, a block, or a whole document while preserving code,
  URLs, diagram source, math, and markup.
- Detect external filesystem changes and refuse to overwrite them silently.

Files stay ordinary files. No import step, no proprietary format — Codex, Claude
Code, Cursor, OpenCode, git, and shell tools consume them unchanged.

## Run a release build

```sh
./scripts/package-release.sh     # builds, verifies, and archives into dist/
```

The **Bump version** GitHub Actions workflow publishes releases from the
default branch. Its default operation is `patch`; it updates the workspace and
lockfile together, commits the change, creates `v<version>`, then calls the
release workflow directly. The direct call matters because a tag pushed with a
workflow's `GITHUB_TOKEN` does not start another workflow run.

The same control also supports `major`, `minor`, `alpha`, `beta`, `rc`, and
`release`. Version changes are delegated to `cargo-release`, and prerelease
steps move forward on one core version:

```text
0.1.1 -> 0.1.2-alpha.1 -> 0.1.2-beta.1 -> 0.1.2-rc.1 -> 0.1.2
```

Repeating a channel increments its sequence. `release` removes the prerelease
suffix, while `patch` on a prerelease promotes that core version to stable,
matching `cargo-release` semantics. The **Release** workflow can also be run for
an existing tag, and a manually pushed `v*` tag triggers it.

Every release runs Clippy and the release-profile workspace tests on native
Linux, macOS, and Windows runners before using `package-release.sh`. Versions
with a prerelease suffix become GitHub prereleases; stable versions become
normal releases. The resulting assets are host-native archives, not signed
installers or a notarized macOS application bundle.

Install the pinned release tool to use the same version logic locally:

```sh
cargo binstall cargo-release@1.1.5 --no-confirm --disable-telemetry
cargo release version patch --workspace        # dry run
cargo release version patch --workspace --execute --no-confirm
cargo release version alpha --workspace --execute --no-confirm
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
            explorer · harness · search · document · settings_page
            workspace + tabs / history / web_surface
```

`mt-doc` has no GPUI dependency, so the same model can later drive CLI tooling,
MCP tools, or headless rendering without touching the UI.

## Renderers

| Technology | Backend | Availability |
|---|---|---|
| Mermaid | `mermaid-svg` (pure Rust) | Always |
| D2 | `d2-little` (pure Rust, own layout) | Always |
| LaTeX / math | RaTeX (pure Rust, no JS engine) | Fonts ship beside the binary |
| PlantUML | `plantuml` CLI | Requires a local install (Java) |

Mermaid and D2 need nothing installed. Math needs the KaTeX faces, which the
release archive stages next to the executable — nothing to install, and none of
their bytes are in the binary, because this application embeds no font it can
ship instead. PlantUML has no usable pure-Rust implementation today. When
either dependency is absent, blocks show an install hint rather than failing. A
renderer that errors or panics produces an inline diagnostic with the original
source preserved — never a crash.

Adding a technology is a registration: implement `BlockRenderer`, register it,
and add the fence language to `DiagramKind::from_lang`. No parser or view change.

## Translation

`TranslationService` is a trait in `mt-doc`; the client lives in `mt-app` and is
built on [`genai`](https://crates.io/crates/genai). The document engine decides
*what* is translatable — prose only, never code, URLs, link targets, frontmatter
keys, diagram source, math, or block markup — and the provider only ever sees
prose fragments.

Providers are named for the wire format they speak, not the vendor, so any
compatible server (vLLM, Ollama, OpenRouter, LM Studio, Azure) works by pointing
the base URL at it:

| Provider | Endpoint | Key |
|---|---|---|
| Anthropic Messages | `/v1/messages` | `ANTHROPIC_API_KEY` |
| OpenAI Chat Completions | `/v1/chat/completions` | `OPENAI_API_KEY` |
| OpenAI Responses | `/v1/responses` | `OPENAI_API_KEY` |

Configure the provider, key, model, base URL, and target language in Settings
(`Ctrl/Cmd+,`). A key set there outranks the environment. With no key anywhere,
translation reports that it is unconfigured rather than pretending to run —
there is no offline stand-in that could stand in for a translation without lying
about it.

A base URL must include the version segment — `http://localhost:11434/v1`, not
`http://localhost:11434`. Only the leaf path is appended, so one without it
reaches an endpoint that is not there.

## Settings

Settings are a TOML file, written when something changes:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\markturbo\settings.toml` |
| macOS | `~/Library/Application Support/markturbo/settings.toml` |
| Linux | `$XDG_CONFIG_HOME/markturbo/settings.toml`, else `~/.config/markturbo/settings.toml` |

TOML because the file is meant to be opened: it takes comments, and it does not
fail on a trailing comma. `$MARKTURBO_CONFIG_DIR` overrides the directory, which
is what keeps a portable install — and the test suite — out of the real one.

Every environment variable the app reads:

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | Key for the Anthropic Messages provider, if none is set in Settings |
| `OPENAI_API_KEY` | Key for both OpenAI providers, likewise |
| `MARKTURBO_TRANSLATE_MODEL` | Model id, if none is set in Settings |
| `MARKTURBO_CONFIG_DIR` | Overrides where `settings.toml` lives |
| `MT_MATH_FONT_DIR` | Folder holding the KaTeX `.ttf` faces, when they are neither beside the executable nor installed |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug` |

A value in Settings always outranks the environment. Nothing else is read —
there is no hidden configuration.

## Security

MDX can contain executable code, and a local HTML file can reference its own
directory. Every document markturbo renders itself is served to the WebView from
an opaque `data:` origin under a CSP that blocks **all** network access;
`Restricted` — the default for every document — also blocks scripts.

Trust is explicit and per-document, and it means something different for each of
the two kinds:

| | Restricted (default) | Trusted |
|---|---|---|
| Markdown / MDX | `data:` origin, no scripts, no network | `data:` origin, scripts run, still no network |
| HTML | `data:` origin with an injected CSP | loaded from `file://` |

The HTML row is the one to know: a trusted `.html` is loaded from disk so its
relative images and stylesheets resolve, which is the whole reason to trust one
— and that gives it a real origin rather than an opaque one. Trusting is never
implicit, and no trust level lets a document markturbo renders reach the network.

## Keyboard

| Key | Action |
|---|---|
| `Ctrl/Cmd+O` | Open folder |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl/Cmd+,` | Settings |
| `Ctrl/Cmd+F` | Find in editor |
| `Ctrl/Cmd+Shift+F` | Search the workspace |
| `Ctrl/Cmd+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl/Cmd+B` | Side panel |
| `Ctrl/Cmd+Alt+B` | Details panel |
| `Alt+Left` / `Ctrl+Alt+-` | Back |
| `Alt+Right` / `Ctrl+Alt+Shift+-` | Forward |
| `Ctrl/Cmd+Shift+T` | Translate document |
| `Ctrl/Cmd+Shift+L` | Translate selection |
| `Ctrl/Cmd+Shift+B` | Translate block at cursor |

## Tests

```sh
uv run scripts/test_release_automation.py
cargo test --release
```

452 tests across 11 binaries, over checked-in fixtures in `fixtures/`: Markdown
constructs including CJK and Unicode, valid and invalid examples of each diagram
technology, MDX (Markdown-only, components, invalid, untrusted), skills (valid,
missing metadata, malformed YAML, nested, multiple roots, name collisions),
filesystem round-trips, and ~10K/~100K-line performance documents.

`--release` is the gate rather than a preference: two performance tests assert
wall-clock bounds that a debug build cannot meet.

Measurement lives in `scripts/` — memory, idle CPU, child windows and hit
testing against a running build. See `scripts/README.md`.

## Contributing

[AGENTS.md](AGENTS.md) is how work gets done here: what to measure and how, what
a test owes you, and the handful of structural rules whose violation breaks
something guaranteed elsewhere. `CLAUDE.md` symlinks to it.

## License

Apache-2.0
