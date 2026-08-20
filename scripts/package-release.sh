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

`markturbo --help` lists every option.

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

## Optional: real translation

Translation works offline out of the box using the **Echo** provider, which
marks each translatable fragment so you can see exactly what would be sent
without any credentials.

For real translation:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
export MARKTURBO_TRANSLATE_TO=zh          # optional, default zh
export MARKTURBO_TRANSLATE_MODEL=...      # optional
```

## Keyboard

| Key | Action |
|---|---|
| `Ctrl/Cmd+O` | Open folder |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+W` | Close tab |
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
    # `tar` ships with Windows 10+; prefer zip for a familiar double-click.
    if command -v powershell.exe >/dev/null 2>&1; then
      powershell.exe -NoProfile -Command \
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
