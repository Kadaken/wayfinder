#!/usr/bin/env bash
# D-394: uninstall must give folders back to the app that had them before
# Wayfinder, or drop Wayfinder's line when nothing was recorded, and never
# touch a default the user set to something else since. Runs the real
# give_back_folder_app (cut out of install.sh) in a fake HOME with a stand-in
# xdg-mime that only records its arguments. Usage:
#   packaging/test-folder-default-d394.sh [install.sh] [scratch-parent]
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
script="${1:-$here/install.sh}"
parent="${2:-${TMPDIR:-/tmp}}"
fake="$(mktemp -d "$parent/wf-d394-test.XXXXXX")"
trap 'rm -rf "$fake"' EXIT
mkdir -p "$fake/bin"
cat > "$fake/bin/xdg-mime" <<EOF
#!/bin/sh
echo "\$*" >> "$fake/xdg-mime.log"
EOF
chmod +x "$fake/bin/xdg-mime"
PATH="$fake/bin:$PATH"
wf_desktop=com.kadaken.wayfinder.desktop
dry=0
say() { :; }
run() { "$@"; }
body="$(sed -n '/^give_back_folder_app() {/,/^}/p' "$script")"
[ -n "$body" ] || { echo "FAIL: install.sh has no give_back_folder_app"; exit 1; }
eval "$body"
fails=0
check() { if eval "$2"; then echo "PASS: $1"; else echo "FAIL: $1"; fails=$((fails + 1)); fi; }
setup() {
  rm -rf "$fake/case"; mkdir -p "$fake/case"; : > "$fake/xdg-mime.log"
  mimeapps="$fake/case/mimeapps.list"
  prev_folder_app="$fake/case/previous-folder-app"
  printf '[Default Applications]\ninode/directory=%s;\ntext/plain=gedit.desktop;\n' "$1" > "$mimeapps"
}

# 1. The app recorded at install time gets folders back.
setup "$wf_desktop"; echo org.kde.dolphin.desktop > "$prev_folder_app"
give_back_folder_app
check "recorded app gets folders back" \
  'grep -qx "default org.kde.dolphin.desktop inode/directory" "$fake/xdg-mime.log"'
check "record removed after use" '[ ! -e "$prev_folder_app" ]'

# 2. Nothing recorded: Wayfinder's line is dropped, other lines stay.
setup "$wf_desktop"
give_back_folder_app
check "no record: Wayfinder line dropped" '! grep -q "^inode/directory=" "$mimeapps"'
check "no record: other lines kept" 'grep -q "^text/plain=gedit.desktop;" "$mimeapps"'
check "no record: xdg-mime not called" '[ ! -s "$fake/xdg-mime.log" ]'

# 3. The record names Wayfinder itself: treated as no record.
setup "$wf_desktop"; echo "$wf_desktop" > "$prev_folder_app"
give_back_folder_app
check "self record: line dropped, xdg-mime not called" \
  '! grep -q "^inode/directory=" "$mimeapps" && [ ! -s "$fake/xdg-mime.log" ]'

# 4. The user moved folders to another app since: left alone.
setup org.gnome.Nautilus.desktop; echo org.kde.dolphin.desktop > "$prev_folder_app"
give_back_folder_app
check "user's own default left alone" \
  'grep -qx "inode/directory=org.gnome.Nautilus.desktop;" "$mimeapps" && [ ! -s "$fake/xdg-mime.log" ]'
check "stale record removed" '[ ! -e "$prev_folder_app" ]'

[ "$fails" = 0 ] && echo "ALL PASS" || { echo "$fails FAILED"; exit 1; }
