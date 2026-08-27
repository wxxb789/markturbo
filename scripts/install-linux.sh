#!/usr/bin/env bash
# Install an extracted Linux release into the current user's XDG data home.
set -euo pipefail

if [ "$#" -gt 1 ]; then
  echo "usage: $0 [release-directory]" >&2
  exit 2
fi

SOURCE="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
SOURCE="$(cd "$SOURCE" && pwd)"
DATA_HOME="${XDG_DATA_HOME:-${HOME:?HOME must be set}/.local/share}"
APP_HOME="$DATA_HOME/markturbo"
INSTALL_ROOT="$APP_HOME/app"
DESKTOP_ID="io.github.wxxb789.markturbo.desktop"
DESKTOP_DESTINATION="$DATA_HOME/applications/$DESKTOP_ID"
ICON_DESTINATION="$DATA_HOME/icons/hicolor/512x512/apps/io.github.wxxb789.markturbo.png"

for required in \
  "$SOURCE/markturbo" \
  "$SOURCE/fonts" \
  "$SOURCE/sample" \
  "$SOURCE/docs" \
  "$SOURCE/share/applications/$DESKTOP_ID.in" \
  "$SOURCE/share/icons/hicolor/512x512/apps/io.github.wxxb789.markturbo.png"; do
  [ -e "$required" ] || {
    echo "error: release is missing $required" >&2
    exit 1
  }
done

# Desktop Entry quoting is separate from shell quoting. The generated absolute
# path makes the launcher independent of whichever directories happen to be on PATH.
desktop_exec_path() {
  printf '"'
  printf '%s' "$1" | sed -e 's/[\\`"$]/\\&/g' -e 's/%/%%/g'
  printf '"'
}

render_desktop_entry() {
  local template="$1"
  local executable="$2"
  local line rendered=0

  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      'Exec=@MARKTURBO_EXECUTABLE@ %F')
        printf 'Exec=%s %%F\n' "$executable"
        rendered=1
        ;;
      *) printf '%s\n' "$line" ;;
    esac
  done < "$template"

  [ "$rendered" -eq 1 ] || {
    echo "error: desktop template is missing its Exec placeholder" >&2
    return 1
  }
}

PUBLISHING=0
COMMITTED=0
STAGING=""
LOCK="$DATA_HOME/.markturbo-install.lock"
LOCK_HELD=0
declare -a PUBLISHED_DESTINATIONS=()
declare -a PREVIOUS_DESTINATIONS=()
declare -a BACKUPS=()

cleanup_staging() {
  if [ -n "$STAGING" ] && ! rm -rf "$STAGING"; then
    echo "error: could not clean staging directory $STAGING" >&2
    return 1
  fi
}

rollback() {
  local index failed=0

  for ((index = ${#PUBLISHED_DESTINATIONS[@]} - 1; index >= 0; index--)); do
    if ! rm -rf "${PUBLISHED_DESTINATIONS[index]}"; then
      echo "error: could not remove incomplete ${PUBLISHED_DESTINATIONS[index]}" >&2
      failed=1
    fi
  done
  for ((index = ${#PREVIOUS_DESTINATIONS[@]} - 1; index >= 0; index--)); do
    if [ ! -e "${BACKUPS[index]}" ]; then
      continue
    fi
    if [ -e "${PREVIOUS_DESTINATIONS[index]}" ]; then
      echo "error: could not restore ${PREVIOUS_DESTINATIONS[index]} because it still exists" >&2
      failed=1
    elif ! mv "${BACKUPS[index]}" "${PREVIOUS_DESTINATIONS[index]}"; then
      echo "error: could not restore ${PREVIOUS_DESTINATIONS[index]}" >&2
      failed=1
    fi
  done
  return "$failed"
}

release_lock() {
  if [ "$LOCK_HELD" -eq 1 ] && ! rmdir "$LOCK"; then
    echo "error: could not remove installation lock $LOCK" >&2
    return 1
  fi
  LOCK_HELD=0
}

on_exit() {
  local status=$?

  trap - EXIT INT TERM
  if [ "$COMMITTED" -eq 0 ] && [ "$PUBLISHING" -eq 1 ]; then
    echo "error: installation interrupted; restoring the previous release" >&2
    if ! rollback; then
      echo "error: rollback failed; preserved backups at $STAGING" >&2
      return 1
    fi
  fi
  if ! cleanup_staging; then
    return 1
  fi
  if ! release_lock; then
    return 1
  fi
  return "$status"
}

on_signal() {
  exit "$1"
}

publish() {
  local staged="$1"
  local destination="$2"
  local backup="$3"
  local backup_index published_index

  if [ -e "$destination" ]; then
    backup_index=${#PREVIOUS_DESTINATIONS[@]}
    PREVIOUS_DESTINATIONS[backup_index]="$destination"
    BACKUPS[backup_index]="$backup"
    if ! mv "$destination" "$backup"; then
      return 1
    fi
  fi
  published_index=${#PUBLISHED_DESTINATIONS[@]}
  PUBLISHED_DESTINATIONS[published_index]="$destination"
  if ! mv "$staged" "$destination"; then
    return 1
  fi
  return 0
}

# Keep staged files under DATA_HOME so every publication move stays a rename.
install -d "$DATA_HOME"
trap on_exit EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
# mkdir is atomic, unlike checking for a lock file and creating it afterward.
if ! mkdir "$LOCK"; then
  echo "error: another markturbo installation is already running: $LOCK" >&2
  exit 1
fi
LOCK_HELD=1
STAGING="$(mktemp -d "$DATA_HOME/.markturbo-install.XXXXXX")"
STAGED_APP="$STAGING/app"
STAGED_DESKTOP="$STAGING/$DESKTOP_ID"
STAGED_ICON="$STAGING/io.github.wxxb789.markturbo.png"
BACKUP_ROOT="$STAGING/previous"

install -d "$STAGED_APP" "$BACKUP_ROOT"
install -m 755 "$SOURCE/markturbo" "$STAGED_APP/markturbo"
cp -R "$SOURCE/fonts" "$SOURCE/sample" "$SOURCE/docs" "$STAGED_APP/"
for document in README.md LICENSE RUNNING.md; do
  [ -f "$SOURCE/$document" ] && cp "$SOURCE/$document" "$STAGED_APP/"
done
render_desktop_entry "$SOURCE/share/applications/$DESKTOP_ID.in" \
  "$(desktop_exec_path "$INSTALL_ROOT/markturbo")" > "$STAGED_DESKTOP"
cp "$SOURCE/share/icons/hicolor/512x512/apps/io.github.wxxb789.markturbo.png" "$STAGED_ICON"

install -d "$APP_HOME" "$DATA_HOME/applications" "$DATA_HOME/icons/hicolor/512x512/apps"
PUBLISHING=1
if ! publish "$STAGED_APP" "$INSTALL_ROOT" "$BACKUP_ROOT/app" \
  || ! publish "$STAGED_DESKTOP" "$DESKTOP_DESTINATION" "$BACKUP_ROOT/desktop" \
  || ! publish "$STAGED_ICON" "$ICON_DESTINATION" "$BACKUP_ROOT/icon"; then
  echo "error: installation failed; restoring the previous release" >&2
  exit 1
fi
COMMITTED=1

echo "Installed markturbo to $INSTALL_ROOT"
