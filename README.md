# markturbo

A native, local-first desktop workspace for the Markdown artifacts that humans
and AI agents exchange: `README.md`, `AGENTS.md`, `CLAUDE.md`, `SKILL.md`,
prompts, specs, plans, and MDX.

> This is not a Markdown editor with AI features. It is a native workspace for
> Markdown as the interface between humans and AI agents.

The current user, product promise, capability roles, data policy, and success
thresholds are defined in the owner-approved [Product Contract](PRODUCT.md).

## Download and install

Windows 11 x64 is the only public-quality release target. Windows 10 x64,
macOS, and Linux remain compatibility-only and experimental; this release does
not offer a public-quality download for them.
Clean-machine acceptance and a short Review demonstration recording are pending;
the instructions below describe the release path, not completed acceptance.

### Download

Open [GitHub Releases](https://github.com/wxxb789/markturbo/releases) and read the
release's platform, acceptance, and signing status before downloading. The
Windows release path uses one Inno Setup installer,
`MarkTurbo-Setup-<version>.exe`, with a matching
`MarkTurbo-Setup-<version>.exe.sha256` sidecar in `sha256sum` format and
`release.json` recording version, channel, signing state, and payload inventory.
Do not substitute an older portable executable for this installer path.

Download the installer and its checksum sidecar into the same directory. In
PowerShell, replace `<version>` with the release version, then compare the
displayed SHA-256 with the sidecar and the release page:

```powershell
$version = '<version>'
Get-FileHash -Algorithm SHA256 ".\MarkTurbo-Setup-$version.exe"
Get-Content ".\MarkTurbo-Setup-$version.exe.sha256"
```

Alternatively, in a shell with `sha256sum`, replace `<version>` and verify the
sidecar directly:

```sh
sha256sum --check 'MarkTurbo-Setup-<version>.exe.sha256'
```

Do not install if the hashes differ. Stable releases require Authenticode
signing. A prerelease may be unsigned, but its release page must explicitly say
**unsigned**; never treat an unsigned prerelease as signed. A self-signed test
prerelease is labeled separately and is not trusted by Windows until its
certificate is installed manually on a test machine.

### Install

Run the verified installer. The per-user installation requires no administrator
rights and uses `%LOCALAPPDATA%\Programs\MarkTurbo`. It creates a Start menu
shortcut, registers Shell App Paths, and provides an uninstaller. Launch
**MarkTurbo** from the Start menu; no terminal or developer tools are needed for
this release path. This is the first installer-based release; earlier portable
executables are not an upgrade path.

Uninstall removes application-owned installation files, not user Markdown,
settings, or recovery data. Those live outside the installation directory and
survive uninstall; there is no silent user-data deletion. A running MarkTurbo
instance is closed before its application-owned files are removed. No
compatibility with older portable distributions or untested upgrade paths is
claimed.

## First Review

1. Choose **Open Folder** to open your Markdown workspace, or choose **Open
   Bundled Sample** on the welcome screen to explore the embedded sample.
2. Open an artifact, choose **New** and type a rough prompt or plan, or choose
   **Paste** to start from clipboard Markdown. Save new work as an ordinary
   Markdown file when you are ready.
3. Open **Settings**. Set the provider wire format, model, and API base URL;
   enter the credential through the credential controls and choose secure
   storage or session-only use. Do not paste the key into a document, screenshot,
   or settings file. See [Translation](#translation) for wire formats and
   endpoint configuration shared by the model operations.
4. Open **Context** to inspect Effective Agent Context. Select only the named
   sources you want included. The verified profile is
   `codex-agents-md-2026-08-29`; other discovered harness instructions are
   inventory-only, not verified applicable context.
5. Choose **Review**, select the artifact lens and document or selection scope,
   and inspect the outbound disclosure before choosing **Send to model**. Check
   the endpoint and every listed source; cancel if the scope is not what you
   intend. Without a configured model, local editing remains available and
   Review explains what configuration is missing.
6. Read the findings, assumptions, and clarification questions. To revise,
   answer the relevant questions, choose **Prepare revision**, inspect its
   disclosure, and choose **Send revision** only if you agree. Inspect each
   proposed change, approve or reject it individually, then choose **Apply
   approved**. Nothing reaches the buffer automatically; save only the result
   you want to keep.

## Privacy

Opening, editing, rendering, searching, discovering Skills, resolving Effective
Agent Context, and creating recovery checkpoints stay local. There are no
background model requests, content telemetry, training uploads, hidden workspace
scans, or account requirements. Files remain ordinary files on your machine.

Review and Translation contact the configured endpoint only when you explicitly
start the operation and confirm its disclosure. A local loopback endpoint keeps
the model request on this machine; a hosted endpoint receives it remotely.
Before a request, the app shows local/remote status, the provider wire format,
the normalized endpoint identity, and selection, document, or document plus
named Effective Agent Context scope. It explains that protocol framing and the
listed source content cross that boundary and, for a hosted endpoint, leave the
device. Consent is specific to the operation, endpoint, and displayed content;
a material change requires confirmation again.

For an Agent Skill package, the disclosure lists each included file's normalized
relative path, byte size, and inclusion reason. Unlisted files do not leave the
device. If a normally in-scope supporting file is missing from the package,
Review labels the result partial rather than claiming the whole package was
reviewed.

On Windows, persistent credentials live in **Windows Credential Manager**, not
plaintext settings; session-only credentials stay in memory. Environment keys
are used only for their vendor-default endpoint unless you explicitly authorize
that exact custom endpoint. Secure or session credentials outrank environment
keys. If secure storage is unavailable, the alternatives are environment or
session-only credentials, never a plaintext fallback.

Windows settings live at `%APPDATA%\markturbo\settings.toml`. Local runtime
data live under `%LOCALAPPDATA%\markturbo`: current-user DPAPI-encrypted recovery
checkpoints in `recovery\`, and application logs in `logs\markturbo-<pid>.log`.
`MARKTURBO_CONFIG_DIR` and `MARKTURBO_DATA_DIR` override those respective roots;
see [Settings](#settings) for all platform paths and environment variables.
Credentials and request bodies are excluded from settings, recovery, logs,
process arguments, errors, screenshots, fixtures, and metrics.

## Screenshots

Goal 09 release-build visual matrix contact sheets show the app at
[default](docs/evidence/goal-09/final5/contact-default.png),
[minimum](docs/evidence/goal-09/final5/contact-minimum.png), and
[wide](docs/evidence/goal-09/final5/contact-wide.png) viewports. These are
screenshots, not a Review demonstration recording or clean-machine acceptance
evidence. See the [visual acceptance status](docs/goal-09-acceptance.md).

## What it does

- Open a directory as a workspace; browse it in a native file tree.
- Start a new document or paste Markdown from the clipboard without creating a file first.
- Open Markdown, MDX, and agent artifacts in tabs.
- Open the Context panel to inspect discovered Skills, instruction files, and
  the applicable Effective Agent Context chain for the active document.
- Read in **Source / Native / Web / Split·Native / Split·Web** layouts.
- Render Mermaid, D2, PlantUML, and LaTeX from document source.
- Discover project Skills and agent instruction files, parse and validate
  `SKILL.md` frontmatter, and inspect them in the Context panel. Discovery is
  an inventory, not a claim that every discovered instruction applies. The
  verified Effective Agent Context profile, `codex-agents-md-2026-08-29`, shows
  the applicable instruction chain and can include explicitly selected sources
  in Review after endpoint/content disclosure. Other harnesses remain
  inventory-only, and available Skills stay separate from automatic instructions.
- Search workspace file names and document contents.
- Review a document and approve individual Revision changes before they reach
  the buffer; nothing is applied automatically.
- Refuse to overwrite a file that changed on disk after it was opened.

Files stay ordinary files. No import step, no proprietary format — Codex, Claude
Code, Cursor, OpenCode, git, and shell tools consume them unchanged.

The guarded in-app Quit applies Save all / Discard all to the buffers dirty when
the dialog is confirmed, including translations completed while it is open.

## Build and release

```sh
uv run --project scripts scripts/mt.py check full
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

The Goal 10 release path requires the locked validation gate, then packages the
Windows installer, checksum sidecar, and `release.json` described above. The
workflow creates or updates a **draft** release; clean-machine acceptance and
owner approval happen before publication. Versions with a prerelease suffix
become GitHub prereleases, and an unsigned prerelease is labeled as such.
Stable versions require Authenticode signing, and missing signing credentials
block publication. Packaging or CI success alone does not establish
clean-machine acceptance; see the [release contract](docs/goals/10-ship-primary-platform-release.md).

Install the pinned release tool to use the same version logic locally:

```sh
cargo binstall cargo-release@1.1.5 --no-confirm --disable-telemetry
cargo release version patch --workspace        # dry run
cargo release version patch --workspace --execute --no-confirm
cargo release version alpha --workspace --execute --no-confirm
```

Or build and run directly:

```sh
nsh run crates/mt-app                         # this machine's native backend
nsh run crates/mt-app --target macos          # macOS (AppKit)
nsh run crates/mt-app --target linux          # Linux (GTK 4)
nsh run crates/mt-app --target windows        # Windows (Win32/Direct2D)
cargo run --release -p mt-app                 # the same app without the nsh CLI
```

Package a standalone app with `nsh package --target windows|macos|linux` from
`crates/mt-app`; the icon, theme and metadata come from `nsh.json`. The first
build compiles the nsh runtime; see [docs/platforms.md](docs/platforms.md) for
the per-platform toolchain.

## Architecture

Built with Rust and [Native Shell (nsh)](https://nsh.dev) 0.4, drawn by each
desktop's native backend: Win32/Direct2D on Windows, AppKit on macOS and GTK 4
on Linux. There is no GPUI or GPUI Kit dependency.

See [docs/architecture.md](docs/architecture.md). The short version:

```text
mt-core (headless, no UI framework)
    document    source, blocks, outline, lossless file I/O and close rules
    workspace   tree, search, watcher, tabs and history state
    agent_artifacts   Skill/instruction discovery and frozen Review package
    review + translate + model   validation, consent and optional transport
    rendering   Mermaid, D2, PlantUML and embedded-font math
    settings + credentials + recovery + runtime_paths
         ▲
         │
mt-app (nsh desktop shell)
    shell + document + review + settings + web + theme + i18n
    nsh signals, native backends and the platform WebView
```

`mt-app` depends on `mt-core`, not vice versa. Core's provider transport is
optional; the desktop enables it. Headless CLI tools use core without a UI framework.

## Renderers

| Technology | Backend | Availability |
|---|---|---|
| Mermaid | `mermaid-svg` (pure Rust) | Always |
| D2 | `d2-little` (pure Rust, own layout) | Always |
| LaTeX / math | RaTeX (pure Rust, no JS engine) | Fonts embedded in the executable |
| PlantUML | `plantuml` CLI | Requires a local install (Java) |

Mermaid, D2, and the KaTeX faces used by math are built into the Windows release
executable. PlantUML has no usable pure-Rust implementation today. When it is
absent, blocks show an install hint rather than failing. A renderer that errors
or panics produces an inline diagnostic with the original source preserved —
never a crash.

Adding a technology is a registration: implement `BlockRenderer`, register it,
and add the fence language to `DiagramKind::from_lang`. No parser or view change.

## Translation

`TranslationService` and the optional provider client live in `mt-core`; the
client uses [`genai`](https://crates.io/crates/genai). The document engine decides
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

Configure the provider, credential, model, base URL, and target language in
Settings. Persistent credentials use Windows Credential Manager;
session credentials remain in memory. Either outranks the environment. Vendor
environment keys apply automatically only to the vendor's default endpoint, and
a custom endpoint requires explicit authorization for that exact identity. With
no credential anywhere, translation reports that it is unconfigured rather than
pretending to run.

A base URL must include the version segment — `http://127.0.0.1:11434/v1`, not
`http://127.0.0.1:11434`. Only the leaf path is appended, so one without it
reaches an endpoint that is not there.

## Settings

- **Local usage counters** are optional and off by default. Settings discloses
  exactly which content-free daily counts are recorded, shows a summary and
  per-day inspection, and provides enable/disable, deliberate JSON export and
  Clear. Counts stay in `usage.json` under the runtime data root for up to 180
  days; nothing is uploaded. Clear deletes local counts, not exported copies.

Settings are a TOML file, written when something changes:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\markturbo\settings.toml` |
| macOS | `~/Library/Application Support/markturbo/settings.toml` |
| Linux | `$XDG_CONFIG_HOME/markturbo/settings.toml`, else `~/.config/markturbo/settings.toml` |

TOML because the file is meant to be opened: it takes comments, and it does not
fail on a trailing comma. `$MARKTURBO_CONFIG_DIR` overrides the directory, which
is what keeps a portable install — and the test suite — out of the real one.

Application-specific environment variables:

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic key for its default endpoint or an explicitly authorized custom endpoint |
| `OPENAI_API_KEY` | OpenAI key for its default endpoint or an explicitly authorized custom endpoint |
| `MARKTURBO_MODEL` | Shared model id, if none is set in Settings |
| `MARKTURBO_TRANSLATE_MODEL` | Legacy model-id fallback |
| `MARKTURBO_CONFIG_DIR` | Overrides where `settings.toml` lives |
| `MARKTURBO_DATA_DIR` | Absolute override for logs, recovery, optional usage counts, and release-sample materialization (the nsh WebView2 profile stays at `%LOCALAPPDATA%\com.markturbo.app\WebView2`) |
| `MT_MATH_FONT_DIR` | Optional complete KaTeX font-directory override for development; incomplete or absent overrides use embedded release fonts |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug` |

A value in Settings always outranks its matching environment variable. There
is no hidden application-specific configuration.

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

The source editor uses native text-editing shortcuts, including find and
undo/redo. Native keyboard and screen-reader acceptance remains part of the
[visual acceptance gates](docs/goal-09-acceptance.md#remaining-acceptance-gates).

## Tests

```sh
uv run --project scripts scripts/mt.py check fast
uv run --project scripts scripts/mt.py check ci
```

The explicit tooling manifest and Rust workspace tests cover Markdown constructs
including CJK and Unicode, renderer and MDX behavior, agent artifacts,
filesystem round-trips, native-harness contracts, release automation, and
checked-in performance fixtures. Test totals are intentionally not documented:
they change with the code and the command output is the evidence.

PR tests use the optimized `ci` profile: timing-sensitive tests need optimization,
but do not need production LTO or single-unit code generation. `check full` keeps
the production release-profile gate for final artifacts. See
[development validation](docs/development.md) for focused commands and when native
acceptance is needed.

Measurement lives in `scripts/` — memory, idle CPU, child windows and hit
testing against a running build. See `scripts/README.md`.

## Contributing

[AGENTS.md](AGENTS.md) is how work gets done here: what to measure and how, what
a test owes you, and the handful of structural rules whose violation breaks
something guaranteed elsewhere. `CLAUDE.md` symlinks to it.

## License

Apache-2.0. Third-party notices, including the embedded KaTeX font license,
are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
