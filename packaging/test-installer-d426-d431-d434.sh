#!/usr/bin/env bash
# D-426, D-431, D-434: installer audit regressions, all in throwaway HOME trees.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$here/.." && pwd)"
parent="${1:-${XDG_CACHE_HOME:-$HOME/.cache}}"
fake="$(mktemp -d "$parent/wf-installer-audit.XXXXXX")"
trap 'rm -rf -- "$fake"' EXIT

make_project() {
  local project="$1"
  mkdir -p "$project/src-tauri/target/release" "$project/src-tauri/icons"
  cp -a "$repo/packaging" "$project/"
  cp -a "$repo/scripts/wayfinder-new-window" "$project/scripts-wayfinder-new-window"
  for icon in 32x32.png 64x64.png 128x128.png 128x128@2x.png icon.png; do
    : > "$project/src-tauri/icons/$icon"
  done
  printf '#!/bin/sh\n' > "$project/src-tauri/target/release/wayfinder-native"
  printf '#!/bin/sh\n' > "$project/src-tauri/target/release/wayfinder-portal"
  chmod +x "$project/src-tauri/target/release/wayfinder-native" \
    "$project/src-tauri/target/release/wayfinder-portal"
  mkdir -p "$project/scripts"
  cp "$project/scripts-wayfinder-new-window" "$project/scripts/wayfinder-new-window"
  chmod +x "$project/scripts/wayfinder-new-window"
}

run_check_test() {
  local home="$fake/check-home" log="$fake/check.log"
  mkdir -p "$home" "$fake/bin"
  : > "$log"
  printf '#!/bin/sh\nprintf "sudo %s\\n" "$*" >> %q\nexit 99\n' "$log" > "$fake/bin/sudo"
  printf '#!/bin/sh\nprintf "dnf %s\\n" "$*" >> %q\nexit 99\n' "$log" > "$fake/bin/dnf"
  chmod +x "$fake/bin/sudo" "$fake/bin/dnf"
  if ! HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
      PATH="$fake/bin:$PATH" bash "$repo/packaging/install.sh" --check --with-codecs --no-portal \
      > "$fake/check.out" 2>&1; then
    echo "FAIL: D-426 --check exited non-zero"
    return 1
  fi
  [ ! -s "$log" ] || { echo "FAIL: D-426 invoked sudo/dnf"; return 1; }
  grep -q '\[MISSING\]' "$fake/check.out" || { echo "FAIL: D-426 had no missing dependency case"; return 1; }
  echo "PASS: D-426 --check never invoked sudo/dnf"
}

run_folder_test() {
  local project="$fake/folder-project" home="$fake/folder-home"
  make_project "$project"
  mkdir -p "$home"
  cat > "$fake/bin/xdg-mime" <<'SH'
#!/bin/sh
set -eu
[ "${1:-}" = default ]
config="${XDG_CONFIG_HOME:?}/mimeapps.list"
[ -d "$(dirname "$config")" ] || exit 21
[ -z "${FAIL_XDG_MIME:-}" ] || exit 22
printf '[Default Applications]\ninode/directory=%s;\n' "$2" > "$config"
SH
  chmod +x "$fake/bin/xdg-mime"
  HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART=1 \
    bash "$project/packaging/install.sh" --no-build --no-portal > "$fake/folder.out"
  grep -qx 'inode/directory=com.kadaken.wayfinder.desktop;' "$home/config/mimeapps.list" \
    || { echo "FAIL: D-431 mimeapps.list missing folder default"; return 1; }
  local failed_home="$fake/folder-failure-home"
  mkdir -p "$failed_home"
  HOME="$failed_home" XDG_CONFIG_HOME="$failed_home/config" XDG_DATA_HOME="$failed_home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART=1 FAIL_XDG_MIME=1 \
    bash "$project/packaging/install.sh" --no-build --no-portal > "$fake/folder-failure.out"
  grep -qx 'Folder default was not set.' "$fake/folder-failure.out" \
    || { echo "FAIL: D-431 did not report xdg-mime failure"; return 1; }
  echo "PASS: D-431 creates config dir, sets default, and reports failure"
}

run_portal_test() {
  local project="$fake/portal-project" home="$fake/portal-home" conf
  make_project "$project"
  mkdir -p "$home/config/xdg-desktop-portal" "$home/data/wayfinder"
  conf="$home/config/xdg-desktop-portal/portals.conf"
  printf '[preferred]\ndefault=*\norg.freedesktop.impl.portal.FileChooser=kde\n' > "$conf"
  printf 'original backup\n' > "$conf.bak"
  HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART=1 \
    bash "$project/packaging/install.sh" --no-build > "$fake/portal-install.out"
  grep -q 'FileChooser=wayfinder' "$conf"
  HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART=1 \
    bash "$project/packaging/install.sh" --uninstall --no-build > "$fake/portal-uninstall.out"
  grep -qx 'org.freedesktop.impl.portal.FileChooser=kde' "$conf" \
    || { echo "FAIL: D-434 did not restore kde chooser line"; return 1; }
  grep -qx 'original backup' "$conf.bak" \
    || { echo "FAIL: D-434 overwrote original backup"; return 1; }
  echo "PASS: D-434 restores chooser line and preserves backup"
}

run_check_test
run_folder_test
run_portal_test
echo "ALL PASS (3 tests)"
