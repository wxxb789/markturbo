# Platform support

The [historical v0.1 product direction](history/v0.1-product-direction.md)
required build paths to be investigated and documented rather than claiming
portability that did not exist. The current [Product Contract](../PRODUCT.md)
names Windows 11 x64 as the only first public-quality platform.

## Release status

| Platform | App | Native rendering | Editor | WebView | Release status |
|---|---|---|---|---|---|
| Windows 11 (x64) | Yes | Yes | Yes | Yes | **Only public-quality and CD target** |
| macOS | Compatibility build | Yes | Yes | Yes | No CD asset |
| Linux (X11 + Wayland) | Compatibility build | Yes | Yes | No | No CD asset |
| FreeBSD | Best effort | Yes | Yes | No | Untested |
| WebAssembly | No | - | - | - | Out of scope |

Windows 11 x64 is the only platform with a public release contract. The release
workflow publishes one raw `markturbo-windows-x64.exe`, with the required fonts
and bundled sample embedded. Other platforms remain useful for compile and
compatibility coverage, not downloadable product artifacts. The Windows
installer and release-channel signing belong to Goal 10. A downloadable macOS
`.app` with Developer ID signing and notarization remains deferred in
[TODO](TODO.md).

### macOS arm64 PR compatibility check

The pull-request workflow has a dedicated `macos-15` arm64 job. It checks the
runner architecture, builds the locked `mt-app` release executable, stages a
copy in a temporary directory, verifies its Mach-O arm64 slice and existing
code signature, prints linked libraries, and launches that unchanged copy with
the content-free startup trace enabled. The check passes only
when the process emits a `markturbo-startup-v1` `first_frame_painted` event
with the job's nonce and process ID within 120 seconds and remains alive for
two seconds afterward. A launch failure, exit during that window (including a
missing dynamic library), window startup failure, or timeout fails the job;
content-free milestones and captured stdout/stderr are printed when available.

This is evidence only that a source-built executable reached its own
first-frame milestone on the hosted CI runner. It is not a screenshot or a
general usability check, and does not verify the binary reported by a user,
other macOS versions or Macs, a `.app` bundle, Developer ID signing,
notarization, or a published macOS artifact. The Windows executable remains
the only public release asset; this check does not add a macOS release
contract. The reported failure to run a macOS arm64 binary on a user's Mac
remains undiagnosed; even a passing first-frame check would not identify that
artifact's failure.

The startup check opens no document, so it does not construct or navigate a
WKWebView. On macOS, failure to obtain the window handle or create the child
WKWebView is shown persistently in the affected Web pane with an explicit Retry
button, without exposing document content or paths. A replacement document
has its own failure identity. Errors are also handled if the pinned Wry API
returns an immediate `Err`, but its `load_url` returns `Ok` after dispatching
the request, and script evaluation does not report its asynchronous `NSError`;
`PageLoadEvent` has no failure variant. A navigation that fails after dispatch
may therefore leave the Web pane blank without a Retry control. A real HTML
Web-preview interaction on a Mac remains unverified by this CI check.

Apple silicon requires a valid executable signature, but the ad-hoc signature
normally created by the linker is not a Developer ID distribution signature.
The CI check validates signature integrity without re-signing; it does not
establish Gatekeeper approval for a downloaded file. macOS bundle staging,
Developer ID signing, notarization and Finder launch are not implemented by
this Windows-only release pipeline. See [Apple's signing requirement](https://developer.apple.com/documentation/macos-release-notes/macos-big-sur-11_0_1-universal-apps-release-notes)
and [notarization contract](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).

To capture the exact failing executable's terminal diagnostics on that Mac,
run this from Terminal after replacing the path with the exact binary that
failed (keep it quoted if it contains spaces):

```sh
artifact="/absolute/path/to/the-exact-binary"
capture_dir="$(mktemp -d "${TMPDIR:-/tmp}/markturbo-launch.XXXXXX")"
file "$artifact"
lipo -archs "$artifact"
codesign --verify --strict --verbose=2 "$artifact"
otool -L "$artifact"
if "$artifact" >"$capture_dir/stdout.txt" 2>"$capture_dir/stderr.txt"; then
  exit_status=0
else
  exit_status=$?
fi
printf 'artifact_path=%s\nexit_status=%s\ncapture_dir=%s\n' \
  "$artifact" "$exit_status" "$capture_dir"
printf '%s\n' '--- stderr ---'
cat "$capture_dir/stderr.txt"
```

Save the printed artifact path, exit status, and stderr verbatim. If macOS
shows a dialog, record its exact text or a screenshot before dismissing it and
include that with the capture; a Finder-launched dialog may not write its text
to Terminal stderr. If the process remains open, close it normally and report
the resulting exit status. These observations help distinguish the reported
failure from the CI check; they do not by themselves establish its cause. A
quarantined download can also fail Gatekeeper even when the Mach-O signature
verifies; do not treat `codesign --verify` as a notarization check.

## Where the support comes from

The `Cargo.lock`-selected `gpui-pre-platform` 0.3.6 selects a backend per
target; check its pinned `Cargo.toml` and `src/gpui_platform.rs` rather than
assuming a live upstream Zed checkout:

- `target_os = "macos"` → `gpui_macos`
- `target_os = "windows"` → `gpui_windows`
- `target_os = "linux"` or `"freebsd"` → `gpui_linux`

`gpui-kit` 0.6.6 enables `wayland`, `x11`, `font-kit` and `runtime_shaders`
for its `gpui-pre-platform` dependency on desktop targets.

## Application identity and icon

The stable platform identifier is `io.github.wxxb789.markturbo`.

- Windows embeds the multi-resolution `.ico` in `markturbo.exe` at build time.
- Linux loads a checked-in PNG for its compatibility window.
- macOS has a checked-in `.icns` and `Info.plist.in` template, but the current
  raw-executable build does not apply either or produce a `.app` bundle.

All platform icon forms are derived from
`crates/mt-app/resources/icons/markturbo.png`; run
`uv run --project scripts scripts/mt.py icons` after replacing that 1024 px master.

**gpui-wry** (the WebView) states in its own README:

> Only supports macOS and Windows currently.

`build_as_child` is not gated by a `cfg` — it compiles everywhere and documents
that it *panics* on Linux when `gtk::init` was not called on the thread. A
compile-time gate is therefore ours to impose, which is why the crate is a
target-specific dependency here:

```toml
[target.'cfg(any(target_os = "windows", target_os = "macos"))'.dependencies]
gpui-wry.workspace = true
```

On Linux the Web pane renders an explanation instead of a broken surface.
Everything else — the editor, native Markdown rendering, all four diagram and
math renderers, skills, translation, filesystem safety — is unaffected, because
diagrams are rendered to SVG in Rust and drawn natively, not through a browser.

## Build requirements

### Windows

- Visual Studio Build Tools with the C++ workload (for the MSVC toolchain).
- WebView2 runtime — preinstalled on Windows 11 and on current Windows 10.

```sh
cargo run --release -p mt-app --bin markturbo
```

### macOS

- Xcode Command Line Tools.
- WebView is provided by the system WKWebView; nothing to install.

```sh
cargo run --release -p mt-app --bin markturbo
```

### Linux

`gpui_linux` and its dependencies need the usual desktop development packages.
On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libasound2-dev libfontconfig-dev libvulkan-dev
```

```sh
cargo run --release -p mt-app --bin markturbo
```

Vulkan drivers are required — gpui renders through Vulkan on Linux.

No TLS development package is listed, and that is deliberate: the translation
client is built on `rustls` with the `ring` provider, so nothing links OpenSSL.
The only match for "openssl" in the graph is `openssl-probe`, which reads the
system certificate *paths* and links nothing.

### Windows CD artifact

The release workflow produces exactly `markturbo-windows-x64.exe`. It contains its icon,
KaTeX font resources, and bundled sample; it does not upload archives, docs,
installers, or auxiliary payloads. This is a portable executable artifact, not
an installer and not a signed distribution. Installation and OS registration
are reserved for Goal 10.

## Where settings live

`dirs::config_dir()` decides, so each platform gets its own convention rather
than an XDG answer imposed on all three:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\markturbo\settings.toml` (Roaming) |
| macOS | `~/Library/Application Support/markturbo/settings.toml` |
| Linux | `$XDG_CONFIG_HOME/markturbo/settings.toml`, else `~/.config/markturbo/settings.toml` |

`$MARKTURBO_CONFIG_DIR` overrides all three. The macOS row is the one worth
noting: this used to be `~/.config/markturbo`, which is the XDG answer on a
platform that is not XDG — a file there is invisible to every macOS convention
for finding, backing up, or migrating application data.

## Where runtime data and logs live

Application logs on every platform, plus the Windows WebView2 profile, use
`dirs::data_local_dir()`, separate from the settings above. They are local,
potentially growing data and should neither roam between machines nor be
created beside the executable:

| Platform | Runtime data root |
|---|---|
| Windows | `%LOCALAPPDATA%\markturbo` |
| macOS | `~/Library/Application Support/markturbo` |
| Linux | `$XDG_DATA_HOME/markturbo`, else `~/.local/share/markturbo` |

On Windows, all MarkTurbo instances share the persistent WebView2 profile at
`webview2/` under that root. macOS continues to use WKWebView's system-managed
browser storage. Every platform writes application logs under `logs/`, using
one append-only `markturbo-<pid>.log` file per process so concurrent instances
do not contend for one file. `$MARKTURBO_DATA_DIR` overrides this log root on
every platform and the WebView2 profile root on Windows.

## Optional per-platform tooling

Only PlantUML needs anything installed; Mermaid, D2, and math are pure Rust and
compiled in.

| Platform | Install |
|---|---|
| Windows | `winget install plantuml` (or `scoop install plantuml`) |
| macOS | `brew install plantuml` |
| Linux | `apt install plantuml` |

All require a JRE. When `plantuml` is not on `PATH`, PlantUML blocks show an
install hint inline and the status bar reports the renderer as unavailable. The
rest of the document renders normally.

## Notes and caveats

- **First build is slow.** GPUI is compiled from git source. Expect 10-25
  minutes cold; incremental builds are seconds.
- **The gpui revision is pinned by `Cargo.lock`, not by `rev` in the manifest.**
  `gpui` must be declared with the *same* source specification `gpui-component`
  uses, or Cargo resolves two incompatible copies into the graph and the build
  fails with confusing trait errors. This was hit during development; the
  manifest carries a comment explaining it.
- **Syntax highlighting requires a feature.** `gpui-component` has no default
  features; `tree-sitter-languages` is enabled explicitly here.
- **Headless environments.** The app opens a real window and will not run
  without a display server. gpui logs `unable to get cursor position` in a
  headless Windows session; that is the environment, not a fault.

## Not implemented

- **WebAssembly.** `gpui_web` exists upstream and `gpui-component` ships a WASM
  gallery, but this application's premise is that the filesystem is the source
  of truth. A browser build would need a different storage model, which the
  historical local-first brief and current Product Contract both reject.
- **iOS / Android.** No upstream gpui backend.
