#!/usr/bin/env bash
# Build a distributable markturbo release.
#
# Produces dist/markturbo-<version>-<target>/ containing the binary, the sample
# workspace, and the docs — then archives it. Run from the repo root:
#
#     ./scripts/package-release.sh
#
# The archive is self-contained apart from the optional PlantUML dependency;
# Mermaid, D2, and math renderers are compiled in.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TARGET="$(rustc -vV | sed -n 's/^host: //p')"
NAME="markturbo-${VERSION}-${TARGET}"
OUT="$ROOT/dist/$NAME"

case "$TARGET" in
  *windows*) BIN="markturbo.exe" ;;
  *)         BIN="markturbo" ;;
esac

echo "==> Building $NAME"
cargo build --release -p mt-app

BUILT="$ROOT/target/release/$BIN"
[ -f "$BUILT" ] || { echo "error: $BUILT not found" >&2; exit 1; }

echo "==> Verifying the binary runs"
# `--version` exercises argument handling without needing a display server, so
# this catches a binary that cannot even start.
"$BUILT" --version

echo "==> Staging $OUT"
rm -rf "$OUT"
mkdir -p "$OUT"
cp "$BUILT" "$OUT/"
cp README.md LICENSE "$OUT/" 2>/dev/null || cp README.md "$OUT/"
mkdir -p "$OUT/docs"
cp docs/architecture.md docs/platforms.md "$OUT/docs/"

# The sample workspace is what a new user opens first, so it ships too.
cp -r sample "$OUT/sample"

cat > "$OUT/RUNNING.md" <<'EOF'
# Running markturbo

## Start with the sample workspace

```sh
./markturbo sample          # macOS / Linux
markturbo.exe sample        # Windows
```

Open `README.md` in the file tree and follow it — it walks through the views,
diagrams, skills, and translation.

## Open your own project

```sh
markturbo /path/to/your/repo     # a directory becomes the workspace
markturbo /path/to/NOTES.md      # a file opens with its folder as the workspace
markturbo                        # no argument: the current directory
```

Or drag a file or folder onto the window. A folder becomes the workspace; a
document opens in a tab, adopting its parent folder as the workspace if none is
open yet.

`markturbo --help` lists every option.

## The window

The side panels run the full height of the window; the bar across the top spans
only the document beside them. Left panel: Files, Harness, Outline. Right panel:
the details of whatever is selected on the left. Each collapses from its button
in the bar, or with a key:

| Key | Panel |
|---|---|
| `Ctrl/Cmd+B` | The side panel |
| `Ctrl/Cmd+Alt+B` | The details panel |

Drag either edge to resize. The bar holds the open document tabs and the
commands — hover a tab for its full path; right-click one to copy that path, or
the path relative to the open folder. The empty space beside the tabs drags the
window, as a title bar should.

Tabs follow the editor convention you already know. A **single click** in the
file tree or the Harness panel opens a *preview* tab — shown in italics, reusing
one slot, replaced by the next single click — so browsing a tree does not leave
forty tabs behind. A **double click** pins it. A preview with unsaved edits is
never replaced.

## Views

The **View** dropdown in the document toolbar picks one of five layouts:

| Layout | Shows |
|---|---|
| Source | The editor alone |
| Native | GPUI-rendered preview — the fast path |
| Web | WebView preview — the compatibility path |
| Split · Native | Editor beside the native preview |
| Split · Web | Editor beside the WebView preview |

Clicking a heading in the Outline jumps there in whichever layout is showing. A
preview-only layout scrolls its preview rather than switching away from it.

## Optional: PlantUML

Mermaid, D2, and LaTeX/math are compiled into the binary and always work.
PlantUML is the one renderer that needs a local install (it requires Java):

| Platform | Install |
|---|---|
| Windows | `winget install plantuml` |
| macOS | `brew install plantuml` |
| Linux | `sudo apt install plantuml` |

Without it, PlantUML blocks show an install hint inline and the status bar notes
the renderer as unavailable. Nothing else is affected.

## Translation

Translation needs an API key. Set one in Settings, or export it:

```sh
export ANTHROPIC_API_KEY=sk-ant-...   # Anthropic Messages
export OPENAI_API_KEY=sk-...          # both OpenAI formats
```

A key set in Settings takes priority over the environment. The environment
remains the option if you would rather a key never touched disk — the settings
file stores it as plain text.

Three wire formats are supported, which between them cover essentially every
hosted and self-hosted endpoint:

| Provider | Endpoint | Key |
|---|---|---|
| Anthropic Messages | `/v1/messages` | `ANTHROPIC_API_KEY` |
| OpenAI Chat Completions | `/v1/chat/completions` | `OPENAI_API_KEY` |
| OpenAI Responses | `/v1/responses` | `OPENAI_API_KEY` |

Set **Base URL** to reach an OpenAI-compatible server — vLLM, Ollama,
OpenRouter, LM Studio, Azure. The wire format is the same, so picking Chat
Completions and pasting a URL is all that is needed.

Without a key, the Translate command reports that one is missing rather than
doing something that looks like a translation and is not.

## Settings

`Ctrl/Cmd+,` opens Settings. Changes save immediately to:

| Platform | Location |
|---|---|
| Windows | `%APPDATA%\markturbo\settings.json` |
| macOS / Linux | `$XDG_CONFIG_HOME/markturbo/settings.json`, else `~/.config/markturbo/settings.json` |

Set `MARKTURBO_CONFIG_DIR` to put it somewhere else — useful for a portable
install. The file is plain JSON and safe to edit by hand; if it cannot be
parsed, markturbo logs a warning, starts with defaults, and leaves your file
alone rather than overwriting it.

**Appearance** — twelve preset themes, six light and six dark:

| Light | Dark |
|---|---|
| Light, Notion, Bear | Dark, Midnight, Nord |
| Elegant, Sepia, Writer | Gruvbox, Solarized Dark, Dracula |

You pick one for each mode, because **Mode** defaults to *System* and keeps
following the OS while running — a machine that switches to dark at sunset takes
the app with it, landing on the dark preset you chose. Writer is monospace;
Sepia and Elegant are serif. The theme drives both the window and the Web
preview, so the two never disagree.

**Language** — the interface itself, in English or 简体中文. Separate from the
translation target, which is about documents.

**Editor → Sync scrolling** — off by default. On, the preview follows the editor
in Split view. The mapping is proportional, so a document with one tall diagram
moves further than the eye expects. It drives the Web preview; the native
preview does not expose a scroll handle yet.

## The Harness panel

Skills and instruction files are both agent artifacts discovered from the same
harness conventions, so they share one panel with two sections.

**Skills** lists every `SKILL.md` found across ~75 workspace conventions
(`skills/`, `.agents/skills`, `.claude/skills`, …) and, when *Include global
skills* is on, every harness's global directory. A skill reachable by several
paths — a junctioned `~/.claude/skills` — appears once, with the other paths
shown as links. Group by origin, harness, or validation status; the last puts
what needs fixing first. Selecting one shows its metadata and any validation
errors, each with the line it came from.

**Instructions** lists `AGENTS.md`, `CLAUDE.md`, Cursor rules, and scoped
`*.instructions.md` — the files a harness reads unprompted.

## Keyboard

| Key | Action |
|---|---|
| `Ctrl/Cmd+O` | Open folder |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl/Cmd+,` | Settings |
| `Ctrl/Cmd+B` | Toggle the side panel |
| `Ctrl/Cmd+Alt+B` | Toggle the details panel |
| `Ctrl/Cmd+F` | Find in the editor |
| `Ctrl/Cmd+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl/Cmd+Shift+T` | Translate document |
| `Ctrl/Cmd+Shift+L` | Translate selection |
| `Ctrl/Cmd+Shift+B` | Translate the block at the cursor |

## Troubleshooting

**Nothing happens / no window.** markturbo needs a display server; it will not
run over a plain SSH session. Set `RUST_LOG=debug` to see what it is doing.

**A diagram shows an error box.** That is intended: renderer failures produce an
inline diagnostic with your original source preserved, rather than crashing.
Check the message for the line number.

**"This file changed on disk".** Something else — an agent, git, another editor
— wrote the file while you had it open. markturbo will not overwrite it
silently; choose *Reload* or *Overwrite* in the banner.
EOF

echo "==> Archiving"
cd "$ROOT/dist"
case "$TARGET" in
  *windows*)
    # Prefer zip for a familiar double-click. PowerShell 7+ first; fall back to
    # Windows PowerShell, then to the `tar` that ships with Windows 10+, so the
    # script still produces an archive on a machine with neither.
    PS=""
    for candidate in pwsh pwsh.exe powershell.exe; do
      if command -v "$candidate" >/dev/null 2>&1; then PS="$candidate"; break; fi
    done
    if [ -n "$PS" ]; then
      "$PS" -NoProfile -Command \
        "Compress-Archive -Path '$NAME' -DestinationPath '$NAME.zip' -Force" >/dev/null
      ARCHIVE="$NAME.zip"
    else
      tar -czf "$NAME.tar.gz" "$NAME"
      ARCHIVE="$NAME.tar.gz"
    fi
    ;;
  *)
    tar -czf "$NAME.tar.gz" "$NAME"
    ARCHIVE="$NAME.tar.gz"
    ;;
esac

echo
echo "==> Done"
echo "    directory: dist/$NAME"
echo "    archive:   dist/$ARCHIVE"
du -h "$ARCHIVE" | awk '{print "    size:      " $1}'
