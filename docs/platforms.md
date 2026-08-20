# Platform support

GOAL §22 asks that build paths be *investigated and documented*, and that
platforms be implemented where the upstream stack supports them reliably —
rather than claiming portability that does not exist.

## Summary

| Platform | App | Native rendering | Editor | WebView | Verified |
|---|---|---|---|---|---|
| Windows 10/11 (x64) | ✅ | ✅ | ✅ | ✅ | **Built and run** |
| macOS | ✅ | ✅ | ✅ | ✅ | Upstream-supported, not run here |
| Linux (X11 + Wayland) | ✅ | ✅ | ✅ | ❌ | Upstream-supported, not run here |
| FreeBSD | ⚠️ | ✅ | ✅ | ❌ | Compiles per upstream `cfg`; untested |
| WebAssembly | ❌ | — | — | — | Out of scope for a local-first app |

"Verified" means what was actually exercised while building this. Only Windows
was: the binary was compiled and launched, a native window opened with a D3D11
device, and the full test suite ran. The other rows report what the upstream
crates declare, not results.

## Where the support comes from

**gpui** (`zed-industries/zed`) selects a backend per target in
`crates/gpui_platform/Cargo.toml`:

- `target_os = "macos"` → `gpui_macos`
- `target_os = "windows"` → `gpui_windows`
- `target_os = "linux"` or `"freebsd"` → `gpui_linux`

`gpui_linux` defaults to both `wayland` and `x11`; this workspace enables both
explicitly, along with `font-kit` and `runtime_shaders`.

**gpui-wry** (the WebView) states in its own README:

> Only supports macOS and Windows currently.

Its `build_as_child` path is compiled only for Windows, macOS, iOS, and Android.
The crate is therefore a target-specific dependency here:

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
cargo run --release
```

### macOS

- Xcode Command Line Tools.
- WebView is provided by the system WKWebView; nothing to install.

```sh
cargo run --release
```

### Linux

`gpui_linux` and its dependencies need the usual desktop development packages.
On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libssl-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libasound2-dev libfontconfig-dev libvulkan-dev
```

```sh
cargo run --release
```

Vulkan drivers are required — gpui renders through Vulkan on Linux.

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
  of truth. A browser build would need a different storage model, which GOAL
  §1.2 rules out for v0.1.
- **iOS / Android.** No upstream gpui backend.
