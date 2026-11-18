# markturbo

A native, local-first desktop workspace for the Markdown artifacts that humans
and AI agents exchange: `README.md`, `AGENTS.md`, `CLAUDE.md`, `SKILL.md`,
prompts, specs, plans, and MDX.

It helps a developer turn rough intent into clear, testable, context-aware
Markdown: it shows what the configured reviewer understood and which agent
instructions apply, asks only questions that could change the outcome, and
applies only the changes the user approves.

The first user, product promise, capability roles, data policy, and success
thresholds are defined in the owner-approved [Product Contract](PRODUCT.md).

## Download and install

Release status is **deferred**. On 2026-11-14 the owner archived the release
sequence as execution deferred, not accepted (see the archived
[Goal 10](docs/goals/archive/10-ship-primary-platform-release.md)). No
installer release is approved or published. Clean-machine acceptance and a
short Review demonstration recording remain pending. Before publication, a
release needs both clean-machine acceptance and owner approval. Read a
release's platform, acceptance, and signing status before downloading it.

Windows 11 x64 is the first public-quality target. The Product Contract also
names macOS 14+ and Linux with GTK 4.14+ as supported targets; each becomes
public-quality only after its own clean-machine acceptance. None has that
evidence today. Windows 10 x64 is compatibility-only: the installer runs there,
but no public-quality claim is made for it.

### What is on the Releases page

[GitHub Releases](https://github.com/wxxb789/markturbo/releases) currently
marks `v0.1.3`, a portable build, as **Latest**; `v0.1.4-alpha.1` is a portable
prerelease. Both predate the installer path and the current UI. Installer
builds (`v0.1.4-alpha.4`, `v0.1.4-alpha.5`) exist only as unpublished drafts.

### Installer path (for a future approved release)

The Windows release workflow produces:

- `MarkTurbo-Setup-<version>.exe`, an Inno Setup installer;
- `MarkTurbo-Setup-<version>.exe.sha256`, in `sha256sum` format;
- `release.json`, recording version, channel, signing state, and payload
  inventory;
- for a self-signed test prerelease only, `MarkTurbo-Setup-<version>.cer`.

Verify the installer before running it:

```powershell
$version = '<version>'
Get-FileHash -Algorithm SHA256 ".\MarkTurbo-Setup-$version.exe"
Get-Content ".\MarkTurbo-Setup-$version.exe.sha256"
```

```sh
sha256sum --check 'MarkTurbo-Setup-<version>.exe.sha256'
```

Do not install if the hashes differ. Stable releases require Authenticode
signing. A prerelease may be unsigned, and its release page must say so; never
treat an unsigned prerelease as signed. A self-signed test prerelease is not trusted by Windows until its certificate is
installed manually.

The installer is per-user and needs no administrator rights. It installs to
`%LOCALAPPDATA%\Programs\MarkTurbo`, creates a Start menu shortcut, registers
Shell App Paths, offers an optional desktop icon, and provides an uninstaller.
Uninstall first closes MarkTurbo instances running from that directory and
stops if it cannot. It removes application files only; Markdown files,
settings, and recovery data live elsewhere and are kept. Portable builds are not
an upgrade path.

## First Review

1. On the welcome screen, choose **Open folder** to open a workspace, **New
   file** or **Paste** to start a document, or **Open sample** (shown when a
   sample is available: the embedded copy in release builds, or the
   repository's `sample/` folder in debug builds). Recent folders and files are
   listed below; one that no longer exists is marked **Missing**. Once a folder
   is open, the welcome gives way to the workspace's home, which lists its
   first files; press `Ctrl+P` to find any file. There is no file tree.
2. Open **⚙ → Settings**. Under **Endpoint**, choose a **Provider**, and
   optionally a **Base URL** and **Model**. Under **Credential**, enter the
   **API key** and choose **Store securely** or **Use for session**. Do not
   paste the key into a document, the settings file, or a screenshot. **Close**
   also saves.
3. Click the file name at the left of the title row (or choose **⚙ →
   Context**) to see the instruction files that apply to this file, in order. Tick **Include in review** only for the sources you
   want sent. The verified profile is `codex-agents-md-2026-08-29`; other
   discovered instructions are inventory only.
4. Choose **Review** in the title row (or `Ctrl+Enter`) to review the whole
   document, or select text in the source editor and choose **⚙ → Review
   selection** to review only that. The lens (Prompt, Specification, Plan,
   Agent Instructions, Agent Skill) is inferred from the file path. An Agent
   Skill is always reviewed as its whole package. The **Send for review?**
   card shows **To** (endpoint, local or remote), **Scope** (**Selection**,
   **Document**, or **Document + context**; a Review started while text is
   selected can switch between them), what **Leaves this device**, what
   **Stays here**, and the **Content total**. A selection is sent without
   Effective Agent Context. Check the endpoint and every listed row; cancel if the
   scope is not what you intend. **Cancel** is the card's default: `Enter` (or
   `Esc`) cancels. On macOS and Linux, Cancel also takes the keyboard focus; on
   Windows no view takes focus, so the keyboard route to send is holding
   `Ctrl+Enter`. To send, press and hold **Hold to send**, or hold
   `Ctrl+Enter`; letting go early shows **Nothing was sent.**
   Without a model, Review shows **No model set up** and **Open settings**.
5. Read the result: the lens, **Understood:** (the goal as the model read it),
   and questions in priority order with **Why it matters:**. Type an answer and
   choose **Answer**, or **Skip**. Then choose **Prepare revision**, check the
   **Send the revision?** card, and hold to send.
6. Each proposed change shows the old text struck through and the
   **Proposed text**. **Accept** writes the change into the buffer at once;
   **Reject** leaves (or puts back) the original text (`A` / `R`). `Ctrl+Z`
   takes the last decision back together with its text; a change whose part
   of the document you edited since can no longer be accepted (**Can't apply —
   this part changed**). In the document, each undecided change's old words stay
   struck through until you decide it.
   **Copy** puts the approved text on the clipboard. **Done** (`Ctrl+Enter`)
   keeps the decisions as written and reports undecided changes as **not
   applied**. Nothing is saved; save with **⚙ → Save**. Every write of an
   accepted or undone change first returns the tab to **Restricted** Web
   scripts.

If you edit the document after sending, the result is marked **Stale — edited
after review** and offers **Review again**.

## Privacy

Opening, editing, rendering, searching, discovering Skills, resolving Effective
Agent Context, and recovery checkpoints stay on the device. There are no
background model requests, content telemetry, training uploads, or accounts.
Files remain ordinary files.

Review and Translation contact the configured endpoint only after you start the
operation and confirm it. Review's card shows the endpoint, whether it is a
**Local model** or **Remote**, whether it uses **HTTP (not encrypted)**, any
proxy, and the content that leaves the device. For an Agent Skill, it lists the
package files; a file whose content is not sent is listed as **Path, size and
hash only**. Every file has its own row; a long manifest scrolls inside the
card while the total and controls stay visible. Translation asks through a native
**Translate** dialog.

Consent covers exactly one operation, endpoint, and the content shown; any
change (for example, switching the scope) rebuilds the card and needs a new
hold, and each consent is used once. Files that are not listed never leave the
device. If a normally in-scope supporting file is missing from a Skill package
(for example, a file `SKILL.md` links to that the package does not hold),
the card lists it under **Stays here** as **not sent with the package** before
you send, and the result is labeled **Partial review**: it does not cover that
file.

Only an IP loopback address (`127.0.0.1`, `[::1]`) counts as local. `localhost`
is treated as a remote host: it requires `https` and may go through a proxy.
Remote requests use `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`
when they are set. Otherwise they use the operating system's proxy setting: the
Windows Internet Settings proxy, or the macOS system proxy. Loopback requests
always bypass any proxy. The card says **may use the configured system proxy**
for remote endpoints and **proxy disabled** for local ones.

Credentials are resolved in this order: session (memory), then persistent
(Windows Credential Manager), then environment. They are never stored in
`settings.toml`. On other platforms there is no secure store and no plaintext
fallback; use a session or environment key. The current UI offers no migration
for a legacy `translate-api-key`: it stays in `settings.toml` untouched and is
not used as a credential. Remove it by hand and enter the key under
**Credential**.

Credentials and request bodies never appear in settings, recovery, logs,
process arguments, errors, screenshots, fixtures, or metrics.

Vendor environment keys apply automatically only to the vendor's default
endpoint. A custom endpoint may use one only if `settings.toml` records that
endpoint's identity as `model-environment-key-identity`; the Settings page has
no control for this today.

Recovery checkpoints are Windows-only and encrypted with current-user DPAPI.
Logs go to stderr and `logs\markturbo-<pid>.log` under the runtime data root;
model client tracing is disabled so response bodies do not reach the log. See
[Settings](#settings) for paths.

## Screenshots

There are no screenshots of the current UI in the repository.

## What it does

- Open a folder as a workspace, or start from **New file** or **Paste** without
  creating a file first. Open files appear as dots in the left rail; find files
  with `Ctrl+P` and search contents with `Ctrl+Shift+F`.
- Read and edit in **Edit**, **Preview**, or **Split** (the default for each
  tab), or render through the WebView with **Web preview** and **Split · Web**;
  the ⚙ menu switches between all five.
- Render Mermaid, D2, PlantUML, and LaTeX math from document source.
- Show the Effective Agent Context chain for the active file and the
  workspace's Skills, and include selected sources in Review.
- Review a document or a selection, and apply only the revision changes you
  accept; each decision can be undone.
- Translate a document's prose into a target language.
- On Windows, recover unsaved changes after a crash: a banner offers
  **Restore** or **Discard** on the next start. Recovery checkpoints exist only
  on Windows.
- Refuse to overwrite a file that changed on disk. Saving over it shows
  **Changed on disk** with **Compare**, **Keep mine**, **Use disk version**, and
  **Save As…**. There is no file watcher; the check happens on save and on
  recovery.
- **Save As…** replaces an existing file once the native save dialog has
  confirmed the overwrite, and refuses a destination that is open in another
  tab. Save writes back in the file's own encoding, byte-order mark, and line
  endings; Save As writes UTF-8 and keeps the line endings and byte-order
  mark.
- Ask **Close without saving?** (**Save**, **Don't save**, **Cancel**) when
  closing a dirty tab, choosing **⚙ → Quit**, or closing the window (its close
  button or `Alt+F4`).
- English and Chinese interface; System, Light, or Dark theme.

No import step and no proprietary format: Codex, Claude Code, Cursor, OpenCode,
git, and shell tools read the files unchanged.

## Build and release

Run locally:

```sh
cargo run --release -p mt-app                 # Rust entry point; no nsh CLI
nsh run crates/mt-app                         # this machine's native backend
nsh run crates/mt-app --target macos          # macOS (AppKit)
nsh run crates/mt-app --target linux          # Linux (GTK 4)
nsh run crates/mt-app --target windows        # Windows (Win32/Direct2D)
```

The binary is `markturbo`. Its entry point embeds `crates/mt-app/theme.json` as
nsh tokens and registers the embedded Martian fonts on the root. A path argument
opens that folder, or opens a file with its parent folder.

`nsh package --target windows|macos|linux`, run from `crates/mt-app`, builds a
standalone app; name, id, version, theme, and icon come from `nsh.json`. The
nsh CLI version must equal the `nsh-core` pin (0.5.1). The release workflow
checks this; local `scripts/mt.py release package` does not. See
[docs/platforms.md](docs/platforms.md) for per-platform toolchains.

The release gate:

```sh
uv run --locked --project scripts scripts/mt.py check full
```

The **Bump version** workflow (manual) releases from the default branch. It
runs `check fast`, runs `cargo release version <bump> --workspace`, copies the
new version into `crates/mt-app/nsh.json`, commits `Cargo.toml`, `Cargo.lock`,
and `nsh.json`, creates the `v<version>` tag, pushes, and then calls the
**Release** workflow directly, because a tag pushed with a workflow's
`GITHUB_TOKEN` does not start another workflow. The default bump is `patch`;
`minor`, `major`, `alpha`, `beta`, `rc`, and `release` are also available.
Prerelease steps move forward on one core version:

```text
0.1.1 -> 0.1.2-alpha.1 -> 0.1.2-beta.1 -> 0.1.2-rc.1 -> 0.1.2
```

Repeating a channel increments its sequence; `patch` on a prerelease promotes
it to stable, as in `cargo-release`.

The **Release** workflow also runs for an existing tag or a pushed `v*` tag. It
checks that the tag is on the default branch and that versions agree, runs
`check full` on windows-2022, packages and verifies the installer, smoke-tests
install and uninstall, and creates a **draft** release. Versions with a `-`
suffix become GitHub prereleases. Stable versions require Authenticode signing.
Packaging or CI success does not establish clean-machine acceptance.

To bump locally with the same tool:

```sh
cargo binstall cargo-release@1.1.5 --no-confirm --disable-telemetry
cargo release version patch --workspace        # dry run
cargo release version patch --workspace --execute --no-confirm
```

Then set the same version in `crates/mt-app/nsh.json`;
`scripts/mt.py release versions` fails when they disagree.

## Architecture

Built with Rust and [Native Shell (nsh)](https://nsh.dev) 0.5.1, drawn by each
desktop's native backend: Win32/Direct2D on Windows, AppKit on macOS, and GTK 4
on Linux. There is no GPUI or GPUI Kit dependency.

```text
mt-core (headless, no UI framework)
    document    source, blocks, outline, lossless file I/O and close rules
    workspace   tree, search, watcher, tabs and history state
    agent_artifacts   Skill/instruction discovery and frozen Review package
    review + translate + model   validation, consent and optional transport
    rendering   Mermaid, D2, PlantUML and embedded-font math
    diagnostic + usage
    settings + credentials + recovery + runtime_paths
         ▲
         │
mt-app (nsh desktop shell)
    app + state + web + i18n + assets + app_paths + logging
    ui/  shell, titlebar, rail, welcome, document, preview, review, glow,
         revision, consent, context, notices, settings,
         tokens, fonts, prims, motion
```

`mt-app` depends on `mt-core`, not the reverse. Core's model transport is an
optional feature (`model-transport`) that the desktop app enables by default.
Colors, sizes, and fonts come from `crates/mt-app/theme.json` and
`ui/tokens.rs`. See [docs/architecture.md](docs/architecture.md) for module
boundaries and the reasons behind them.

## Renderers

| Technology | Fence languages | Backend | Availability |
|---|---|---|---|
| Mermaid | `mermaid` | `mermaid-svg` (pure Rust) | Always |
| D2 | `d2` | `d2-little` (pure Rust) | Always |
| LaTeX / math | `math`, `latex`, `tex`, plus `$$…$$` display blocks | RaTeX (pure Rust), KaTeX fonts | Fonts embedded in the executable |
| PlantUML | `plantuml`, `puml`, `uml` | `plantuml` on `PATH` | Requires a local install |

PlantUML is called as `plantuml -tsvg -pipe`; when it is missing, the block
shows an install hint. A renderer error or panic becomes an inline diagnostic
with the source preserved. `graphviz`, `dot`, `vega`, `vega-lite`, `pikchr`,
`svgbob`, `nomnoml`, and `typst` fences are recognized as diagrams but have no
renderer and show a diagnostic.

Adding a technology means implementing `BlockRenderer`, registering it, and
mapping its fence language in `DiagramKind::from_lang`.

## Translation

Choose **⚙ → Translate**. A native dialog shows what will be sent, with
**Send** and **Cancel**. The whole document is translated into the **Target
language** from Settings (`zh` when empty); the result replaces the buffer if
the document has not changed meanwhile, and nothing is saved.

The document engine sends only prose. Code, URLs, link targets, frontmatter
keys, diagram source, and math stay verbatim. The client is
[`genai`](https://crates.io/crates/genai) in `mt-core`.

Providers are named for the wire format they speak:

| Provider | Default base URL | Request path | Key | Default model |
|---|---|---|---|---|
| Anthropic Messages | `https://api.anthropic.com/v1/` | `messages` | `ANTHROPIC_API_KEY` | `claude-sonnet-5` |
| OpenAI Chat Completions | `https://api.openai.com/v1/` | `chat/completions` | `OPENAI_API_KEY` | `gpt-5` |
| OpenAI Responses | `https://api.openai.com/v1/` | `responses` | `OPENAI_API_KEY` | `gpt-5` |

Another server that speaks one of these formats works by setting **Base URL**,
within these rules:

- The base URL must include the version segment (`http://127.0.0.1:11434/v1`,
  not `http://127.0.0.1:11434`); only the request path is appended. A missing
  trailing `/` is added.
- Plain `http` is allowed only for IP loopback addresses. Use `127.0.0.1` or
  `[::1]` rather than `http://localhost`.
- User info, query strings, and fragments are rejected, so endpoints that need
  a query parameter such as `?api-version=` do not work.

With no credential anywhere, the operation reports that none is available. The
model comes from Settings, then `MARKTURBO_MODEL`, then
`MARKTURBO_TRANSLATE_MODEL`; Review and Translation share it.

## Settings

The Settings page has these sections: **Endpoint**, **Credential**,
**Translation**, **Language**, **Theme**, and **Local usage counters**.

Local usage counters are off by default and record only a fixed set of
content-free daily counts. Settings shows a summary and offers **Inspect
counts**, **Export counts…**, and **Clear counts**. Counts stay in `usage.json`
under the runtime data root for today and the previous 180 UTC days; nothing is
uploaded.

Settings are a TOML file:

| Platform | Settings | Runtime data (logs, recovery, usage, sample) |
|---|---|---|
| Windows | `%APPDATA%\markturbo\settings.toml` | `%LOCALAPPDATA%\markturbo` |
| macOS | `~/Library/Application Support/markturbo/settings.toml` | `~/Library/Application Support/markturbo` |
| Linux | `$XDG_CONFIG_HOME/markturbo/settings.toml`, else `~/.config/markturbo/settings.toml` | `$XDG_DATA_HOME/markturbo`, else `~/.local/share/markturbo` |

The WebView2 profile is at `%LOCALAPPDATA%\com.markturbo.app\WebView2`.

Environment variables the app reads:

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | Key for Anthropic's default endpoint |
| `OPENAI_API_KEY` | Key for OpenAI's default endpoint |
| `MARKTURBO_MODEL` | Model id when Settings has none |
| `MARKTURBO_TRANSLATE_MODEL` | Legacy model-id fallback |
| `MARKTURBO_CONFIG_DIR` | Directory for `settings.toml`; blank means unset |
| `MARKTURBO_DATA_DIR` | Absolute runtime data root; a non-empty invalid value disables runtime data rather than falling back |
| `MARKTURBO_OPEN` | Folder or file to open at start; set from the first command-line argument |
| `MT_MATH_FONT_DIR` | KaTeX font directory override; incomplete or missing directories fall back to the embedded fonts |
| `CODEX_HOME` | Codex home for Effective Agent Context, else `~/.codex` |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug` |
| `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` | Proxy for remote model requests; when unset, the OS proxy setting applies |

`PATH` (and `PATHEXT` on Windows) is searched for `plantuml`. A value in
Settings outranks its matching environment variable.

## Security

The web layouts (**Web preview**, **Split · Web**) hand the page to the
platform WebView as an HTML string with no base URL.

Every tab starts **Restricted**. The web pane shows **Restricted: scripts
disabled** and a **Trust scripts** button; trusting asks **Run scripts in this
document?** and, after **Run scripts**, shows **Trusted: scripts may run**.
Trust applies to that open tab. Accepting or undoing a Revision change
revokes it before the text changes.

| | Restricted (default) | Trusted |
|---|---|---|
| Markdown / MDX | CSP: no network, no scripts | CSP: no network, inline scripts run |
| HTML | Injected CSP: no network, no scripts | Original source, no CSP; scripts run and may reach the network |

The CSP is `default-src 'none'; style-src 'unsafe-inline'; img-src data:;
font-src data:;` plus `script-src 'none'` (Restricted) or `'unsafe-inline'`
(Trusted). Because there is no base URL, relative images and stylesheets in a
trusted HTML file do not resolve. The CSP governs what a page loads, not
top-level navigation such as following a link, and the app does not intercept
navigation.

## Keyboard

"mod" is `Ctrl` on Windows and Linux and `Cmd` on macOS.

| Where | Keys | Action |
|---|---|---|
| Anywhere | mod+`Enter` | Review (not while a review is running, a card is open, or a revision is pending) |
| Anywhere | mod+`O` / mod+`N` | Open folder / new file |
| Anywhere | mod+`Shift+S` | Save As |
| Anywhere | mod+`P` / mod+`Shift+F` | Find a file / search files |
| Anywhere | mod+`W` | Close the tab |
| Anywhere | `Esc` | Close a menu or palette |
| Review result | `J` / `K` | Next / previous finding |
| Review | `Esc` | Cancel a running review, or close |
| Send card | hold `Ctrl+Enter` / `Esc` | Send / cancel |
| Revision, keyboard outside the editor | `A` / `R` / `J` / `K` | Accept / reject / move |
| Revision, keyboard outside the editor | `Ctrl+Z` / `Ctrl+Enter` | Undo the last decision / Done |
| Close guard | `Left` / `Right` / `Tab`, `Enter`, `Esc` | Move, choose, cancel |
| Compare | `J` / `K`, `Esc` | Next / previous difference, close |

There is no `Ctrl+S`; save from the ⚙ menu. The source editor
provides native undo/redo, clipboard, and IME.

The table describes the bindings in the code, not accepted behavior: native
keyboard and screen-reader acceptance is still pending.

## Tests

```sh
uv run --locked --project scripts scripts/mt.py check fast   # whitespace, Python tooling tests
uv run --locked --project scripts scripts/mt.py check doc    # + rustfmt, mt-core tests
uv run --locked --project scripts scripts/mt.py check ci     # + Clippy and workspace tests
```

Every PR runs a fast + format preflight. The Rust jobs run only on non-draft
PRs whose diff touches code: Linux Clippy, mt-core and mt-app tests; mt-app and
mt-core tests on windows-2022; a macOS arm64 release build. Tests use
the optimized `ci` profile; `check full` keeps the production release profile.
Test totals are not documented: the command output is the evidence.

UI tests in `crates/mt-app/tests` drive the app headlessly with
`nsh_testing::Headless`. See [development validation](docs/development.md) for
focused commands.

`check native` runs the native harness's Python unit tests; it is not
acceptance. The four `mt.py accept` harnesses (goal-02, goal-03, goal-06,
goal-07) have been ported to the nsh UI's automation IDs but have not yet run
live against an nsh build, so no acceptance evidence exists for the current UI;
see [docs/verification.md](docs/verification.md#native-harness-status). The
measurement probes in
`scripts/` were also written for the GPUI window structure and have not been
revalidated on nsh.

## Contributing

[AGENTS.md](AGENTS.md) is how work gets done here: what to measure and how, what
a test owes you, and the handful of structural rules whose violation breaks
something guaranteed elsewhere. `CLAUDE.md` symlinks to it.

## License

Apache-2.0. Third-party notices for the embedded Martian Grotesk and Martian
Mono fonts (OFL 1.1) and KaTeX fonts (MIT) are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
