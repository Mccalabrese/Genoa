#!/bin/bash
set -euo pipefail

TARGET_DESKTOP="${1:-}"

if [[ -z "$TARGET_DESKTOP" ]]; then
  echo "Error: Missing session file argument." >&2
  exit 1
fi

if [[ ! -f "$TARGET_DESKTOP" ]]; then
  echo "Error: Upstream session $TARGET_DESKTOP not found." >&2
  exit 1
fi

if [[ "$TARGET_DESKTOP" != /usr/share/wayland-sessions/* ]]; then
  echo "Error: Refusing unsupported session path '$TARGET_DESKTOP'." >&2
  exit 1
fi

SESSION_NAME="$(basename -- "$TARGET_DESKTOP")"

case "$SESSION_NAME" in
*niri.desktop)
  unset WAYLAND_DISPLAY
  unset DISPLAY
  exec /usr/bin/niri
  ;;
*sway.desktop)
  unset WAYLAND_DISPLAY
  unset DISPLAY
  exec /usr/bin/sway
  ;;
*gnome-wayland.desktop | *gnome.desktop)
  unset WAYLAND_DISPLAY
  unset DISPLAY
  exec /usr/bin/gnome-session
  ;;
*)
  echo "Error: Unsupported session '$SESSION_NAME'." >&2
  exit 1
  ;;
esac
