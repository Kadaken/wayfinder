#!/usr/bin/env bash
# D-409g: give_back_folder_app must edit the target of a symlinked
# mimeapps.list, not replace the link with a regular file. Runs the real
# function (cut out of install.sh) against a fake HOME. Usage:
#   packaging/test-mimeapps-symlink.sh [install.sh] [scratch-parent]
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
script="${1:-$here/install.sh}"
parent="${2:-${TMPDIR:-/tmp}}"
fake="$(mktemp -d "$parent/wf-mimeapps-test.XXXXXX")"
trap 'rm -rf "$fake"' EXIT
mkdir -p "$fake/home/.config" "$fake/dotfiles"
wf_desktop=com.kadaken.wayfinder.desktop
printf '[Default Applications]\ninode/directory=%s;\ntext/plain=gedit.desktop;\n' "$wf_desktop" > "$fake/dotfiles/mimeapps.list"
ln -s "$fake/dotfiles/mimeapps.list" "$fake/home/.config/mimeapps.list"
mimeapps="$fake/home/.config/mimeapps.list"
prev_folder_app="$fake/none"
dry=0
say() { :; }
run() { "$@"; }
eval "$(sed -n '/^give_back_folder_app() {/,/^}/p' "$script")"
give_back_folder_app
[ -L "$mimeapps" ] || { echo "FAIL: mimeapps.list is no longer a symlink"; exit 1; }
if grep -q '^inode/directory=' "$fake/dotfiles/mimeapps.list"; then echo "FAIL: target still has the Wayfinder line"; exit 1; fi
grep -q '^text/plain=gedit.desktop;' "$fake/dotfiles/mimeapps.list" || { echo "FAIL: other lines lost"; exit 1; }
echo "PASS: still a symlink, target edited, other lines kept"
