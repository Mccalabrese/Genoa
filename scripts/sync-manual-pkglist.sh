#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKGLIST_FILE="$ROOT_DIR/pkglist.txt"
MANUAL_FILE="$ROOT_DIR/docs/MANUAL_INSTALL.md"
START_MARKER='<!-- PKGLIST:START -->'
END_MARKER='<!-- PKGLIST:END -->'

if [[ ! -f "$PKGLIST_FILE" ]]; then
  echo "Missing pkg list: $PKGLIST_FILE" >&2
  exit 1
fi

if [[ ! -f "$MANUAL_FILE" ]]; then
  echo "Missing manual file: $MANUAL_FILE" >&2
  exit 1
fi

if ! grep -qF "$START_MARKER" "$MANUAL_FILE" || ! grep -qF "$END_MARKER" "$MANUAL_FILE"; then
  echo "Manual file is missing PKGLIST markers." >&2
  exit 1
fi

mapfile -t packages < <(grep -vE '^\s*#' "$PKGLIST_FILE" | sed '/^\s*$/d')

if [[ ${#packages[@]} -eq 0 ]]; then
  echo "No packages found in $PKGLIST_FILE" >&2
  exit 1
fi

chunk_size=10
block_file="$(mktemp)"
out_file="$(mktemp)"
trap 'rm -f "$block_file" "$out_file"' EXIT

{
  echo '```bash'
  echo 'sudo pacman -S --needed --noconfirm \'

  i=0
  total=${#packages[@]}
  while [[ $i -lt $total ]]; do
    line="  "
    j=0
    while [[ $j -lt $chunk_size && $i -lt $total ]]; do
      line+="${packages[$i]} "
      ((i+=1))
      ((j+=1))
    done

    line="${line% }"
    if [[ $i -lt $total ]]; then
      line+=" \\"
    fi
    echo "$line"
  done

  echo '```'
} > "$block_file"

awk -v start="$START_MARKER" -v end="$END_MARKER" -v block="$block_file" '
BEGIN { in_block = 0 }
$0 == start {
  print
  while ((getline line < block) > 0) {
    print line
  }
  in_block = 1
  next
}
$0 == end {
  in_block = 0
  print
  next
}
!in_block { print }
' "$MANUAL_FILE" > "$out_file"

mv "$out_file" "$MANUAL_FILE"
echo "Updated package block in $MANUAL_FILE from $PKGLIST_FILE"
