#!/usr/bin/env bash
# D-426, D-431, D-434, D-449, D-451, D-452: installer audit regressions in throwaway HOME trees.
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

stub_portal_tools() {
  local tool
  for tool in pkill systemctl busctl; do
    cat > "$fake/bin/$tool" <<'SH'
#!/bin/sh
printf '%s %s\n' "${0##*/}" "$*" >> "$WF_PORTAL_CALLS"
SH
    chmod +x "$fake/bin/$tool"
  done
}

run_no_portal_test_d449() {
  local project="$fake/no-portal-project" home="$fake/no-portal-home"
  local prefix="$home/.local" conf="$home/config/xdg-desktop-portal/portals.conf"
  make_project "$project"
  rm "$project/src-tauri/target/release/wayfinder-portal"
  mkdir -p "$prefix/bin" "$home/config/xdg-desktop-portal" "$home/config/environment.d"
  printf 'existing portal config\n' > "$conf"

  HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART= WF_PORTAL_CALLS="$fake/portal.calls" \
    bash "$project/packaging/install.sh" --no-build --no-portal --kde-apps > "$fake/no-portal.out"

  [ ! -s "$fake/portal.calls" ] || { echo "FAIL: D-449 called portal tools"; cat "$fake/portal.calls"; return 1; }
  [ ! -e "$prefix/bin/wayfinder-portal" ]
  grep -qx 'existing portal config' "$conf"
  [ ! -e "$prefix/share/xdg-desktop-portal/portals/wayfinder.portal" ]
  [ ! -e "$prefix/share/dbus-1/services/org.freedesktop.impl.portal.desktop.wayfinder.service" ]
  [ ! -e "$prefix/share/applications/com.kadaken.wayfinder.picker.desktop" ]
  [ ! -e "$home/config/environment.d/95-wayfinder-qt.conf" ]
  [ -x "$prefix/bin/wayfinder-native" ]
  echo "PASS: D-449 --no-portal leaves the portal alone"
}

run_no_portal_upgrade_test_d451() (
  local project="$fake/portal-upgrade-project" home="$fake/portal-upgrade-home"
  local prefix="$home/.local" conf="$home/config/xdg-desktop-portal/portals.conf"
  local previous_choice='org.freedesktop.impl.portal.FileChooser = kde;gtk;'
  local portal_bin="$prefix/bin/wayfinder-portal"
  local portal_service="$prefix/share/dbus-1/services/org.freedesktop.impl.portal.desktop.wayfinder.service"
  local portal_files=(
    "$portal_bin"
    "$portal_service"
    "$prefix/share/xdg-desktop-portal/portals/wayfinder.portal"
    "$prefix/share/applications/com.kadaken.wayfinder.picker.desktop"
  )
  local file missing
  make_project "$project"
  mkdir -p "$(dirname "$conf")"
  printf '[preferred]\ndefault=*\n%s\n' "$previous_choice" > "$conf"
  cp "$conf" "$fake/portal-upgrade-original.conf"

  export HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data"
  export PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART='' WF_PORTAL_CALLS="$fake/portal-upgrade.calls"
  for missing in "" "$portal_bin" "$portal_service"; do
    bash "$project/packaging/install.sh" --no-build --kde-apps > "$fake/portal-upgrade-full.out"
    grep -qx 'org.freedesktop.impl.portal.FileChooser=wayfinder' "$conf"
    for file in "${portal_files[@]}"; do
      [ -f "$file" ]
    done

    if [ -n "$missing" ]; then rm "$missing"; fi
    : > "$WF_PORTAL_CALLS"
    bash "$project/packaging/install.sh" --no-build --no-portal > "$fake/portal-upgrade-no-portal.out"
    for file in "${portal_files[@]}"; do
      if [ -e "$file" ] || [ -L "$file" ]; then
        echo "FAIL: D-451 kept the old portal file ${file##*/}"
        return 1
      fi
    done
    cmp "$fake/portal-upgrade-original.conf" "$conf"
    [ ! -e "$home/config/environment.d/95-wayfinder-qt.conf" ]
    [ ! -e "$home/data/wayfinder/previous-portal-filechooser" ]
    [ -x "$prefix/bin/wayfinder-native" ]
    grep -Fxq '  Removed the earlier Wayfinder file dialog.' "$fake/portal-upgrade-no-portal.out"
    grep -Fxq "pkill -f -- ^$portal_bin( |$)" "$WF_PORTAL_CALLS"
    grep -Fxq 'systemctl --user try-restart xdg-desktop-portal.service' "$WF_PORTAL_CALLS"
  done

  : > "$WF_PORTAL_CALLS"
  bash "$project/packaging/install.sh" --no-build --no-portal > "$fake/portal-upgrade-repeat.out"
  [ ! -s "$WF_PORTAL_CALLS" ] || { echo "FAIL: D-451 restarted the portal on repeat install"; return 1; }
  grep -Fxq "$previous_choice" "$conf"
  echo "PASS: D-451 --no-portal removes the earlier file dialog and restores the chooser line"
)

run_uninstall_summary_test_d449() {
  local home="$fake/portal-home" output="$fake/portal-uninstall.out"
  grep -Fxq "  Settings folder: $home/config/wayfinder" "$output"
  grep -Fxq "  Portal settings backup: $home/config/xdg-desktop-portal/portals.conf.bak" "$output"
  grep -Fxq '  Any codec packages installed with --with-codecs' "$output"
  echo "PASS: D-449 uninstall lists settings, the portal backup, and codec packages it keeps"
}

run_dry_run_folder_test_d452() {
  local project="$fake/dry-project" home="$fake/dry-home"
  make_project "$project"
  mkdir -p "$home"
  HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" \
    PATH="$fake/bin:$PATH" WAYFINDER_INSTALL_NO_RESTART=1 \
    bash "$project/packaging/install.sh" --no-build --no-portal --dry-run > "$fake/dry.out"
  grep -q 'would: xdg-mime default com.kadaken.wayfinder.desktop inode/directory' "$fake/dry.out" \
    || { echo "FAIL: D-452 dry run did not list the folder default"; return 1; }
  [ -z "$(find "$home" -mindepth 1 -print -quit)" ] \
    || { echo "FAIL: D-452 dry run wrote into HOME"; return 1; }
  echo "PASS: D-452 dry run lists the folder default and writes nothing"
}

run_check_test
run_folder_test
run_portal_test
stub_portal_tools
run_no_portal_test_d449
run_no_portal_upgrade_test_d451
run_uninstall_summary_test_d449
run_dry_run_folder_test_d452
echo "ALL PASS (7 tests)"
