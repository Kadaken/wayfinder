#!/usr/bin/env bash
# Wayfinder installer and uninstaller.
#
# Installs the two binaries, the launcher symlink, the New Window helper, both
# desktop entries, both D-Bus service files, the portal backend declaration and
# the icons, with the real install prefix written into every file that needs an
# absolute path. D-Bus activation does not search PATH, so those Exec= lines
# cannot be left as bare command names.
#
#   ./packaging/install.sh                    install for you, under ~/.local
#   ./packaging/install.sh --system           install for every user, /usr/local
#   ./packaging/install.sh --prefix /opt/wf   install somewhere else
#   ./packaging/install.sh --uninstall        remove what this installed
#
# Other flags:
#   --destdir DIR   stage the whole tree under DIR (for distribution packagers)
#   --no-portal     do not install the file-dialog backend or touch portals.conf
#   --no-build      stop instead of building when the binaries are missing
#   --dry-run       print every step, change nothing
#
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 LegendofZito (Kadaken)

set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$here/.." && pwd)"

prefix=""
destdir=""
system=0
with_portal=1
build=1
dry=0
uninstall=0

die() { printf 'wayfinder install: %s\n' "$1" >&2; exit 1; }
say() { printf '%s\n' "$1"; }
run() {
  if [ "$dry" = 1 ]; then printf '  would: %s\n' "$*"; else "$@"; fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --system)     system=1 ;;
    --prefix)     shift; [ $# -gt 0 ] || die "--prefix needs a directory"; prefix="$1" ;;
    --prefix=*)   prefix="${1#*=}" ;;
    --destdir)    shift; [ $# -gt 0 ] || die "--destdir needs a directory"; destdir="$1" ;;
    --destdir=*)  destdir="${1#*=}" ;;
    --no-portal)  with_portal=0 ;;
    --no-build)   build=0 ;;
    --dry-run)    dry=1 ;;
    --uninstall)  uninstall=1 ;;
    -h|--help)    sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)            die "unknown option: $1" ;;
  esac
  shift
done

if [ -z "$prefix" ]; then
  if [ "$system" = 1 ]; then prefix="/usr/local"; else prefix="${HOME}/.local"; fi
fi

bindir="$prefix/bin"
datadir="$prefix/share"
# Where files actually land. DESTDIR only changes the write path, never the
# paths written into the files.
d="$destdir"

apps="$d$datadir/applications"
dbus="$d$datadir/dbus-1/services"
portals="$d$datadir/xdg-desktop-portal/portals"
icons="$d$datadir/icons/hicolor"

conf_home="${XDG_CONFIG_HOME:-$HOME/.config}"
portals_conf="$conf_home/xdg-desktop-portal/portals.conf"

icon_sizes="32x32:32x32.png 64x64:64x64.png 128x128:128x128.png 256x256:128x128@2x.png 512x512:icon.png"

installed_files() {
  printf '%s\n' \
    "$d$bindir/wayfinder" \
    "$d$bindir/wayfinder-native" \
    "$d$bindir/wayfinder-portal" \
    "$d$bindir/wayfinder-new-window" \
    "$apps/com.kadaken.wayfinder.desktop" \
    "$apps/com.kadaken.wayfinder.picker.desktop" \
    "$dbus/org.freedesktop.FileManager1.service" \
    "$dbus/org.freedesktop.impl.portal.desktop.wayfinder.service" \
    "$portals/wayfinder.portal"
  local pair
  for pair in $icon_sizes; do
    printf '%s\n' "$icons/${pair%%:*}/apps/com.kadaken.wayfinder.png"
  done
}

refresh_caches() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    run update-desktop-database "$apps" || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    run gtk-update-icon-cache -q -t -f "$icons" || true
  fi
}

restart_portal() {
  [ -n "$destdir" ] && return 0
  if command -v systemctl >/dev/null 2>&1 && [ -z "${WAYFINDER_INSTALL_NO_RESTART:-}" ]; then
    say "Restarting the desktop portal so the change takes effect now."
    run systemctl --user try-restart xdg-desktop-portal.service || true
  fi
}

# ---------------------------------------------------------------- uninstall
if [ "$uninstall" = 1 ]; then
  say "Removing Wayfinder from $prefix"
  while IFS= read -r f; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    say "  $f"
    run rm -f "$f"
  done < <(installed_files)

  if [ -z "$destdir" ] && [ -f "$portals_conf" ] \
     && grep -q '^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$' "$portals_conf"; then
    say "  dropping the file-dialog line from $portals_conf"
    if [ "$dry" = 1 ]; then
      printf '  would: edit %s\n' "$portals_conf"
    else
      cp -p "$portals_conf" "$portals_conf.bak"
      grep -v '^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$' \
        "$portals_conf.bak" > "$portals_conf"
      say "  previous file kept as $portals_conf.bak"
    fi
  fi

  refresh_caches
  restart_portal
  say "Done. Your own settings in ~/.config/wayfinder were left alone."
  exit 0
fi

# ------------------------------------------------------------------ install
bin_src="$root/src-tauri/target/release"
if [ ! -x "$bin_src/wayfinder-native" ] || [ ! -x "$bin_src/wayfinder-portal" ]; then
  [ "$build" = 1 ] || die "no release binaries in $bin_src and --no-build was given"
  command -v cargo >/dev/null 2>&1 || die "cargo is not installed, so the binaries cannot be built"
  say "Building the release binaries. This takes a minute or two."
  if [ "$dry" = 1 ]; then
    printf '  would: cargo build --release --manifest-path %s\n' "$root/src-tauri/Cargo.toml"
  else
    cargo build --release --manifest-path "$root/src-tauri/Cargo.toml"
  fi
fi

say "Installing Wayfinder into $prefix"

run install -d "$d$bindir" "$apps" "$dbus"
run install -m 0755 "$bin_src/wayfinder-native" "$d$bindir/wayfinder-native"
run install -m 0755 "$bin_src/wayfinder-portal" "$d$bindir/wayfinder-portal"
# The file manager and the file dialog are the same binary under two names, and
# the portal backend looks for wayfinder-native beside itself, so both names
# have to sit in the same directory.
run ln -sf wayfinder-native "$d$bindir/wayfinder"
run install -m 0755 "$root/scripts/wayfinder-new-window" "$d$bindir/wayfinder-new-window"

subst() {
  # $1 source, $2 destination: writes the real bin directory into the file.
  if [ "$dry" = 1 ]; then
    printf '  would: write %s with BINDIR=%s\n' "$2" "$bindir"
  else
    sed "s|@BINDIR@|$bindir|g" "$1" > "$2"
    chmod 0644 "$2"
  fi
}

subst "$here/com.kadaken.wayfinder.desktop"        "$apps/com.kadaken.wayfinder.desktop"
subst "$here/com.kadaken.wayfinder.picker.desktop" "$apps/com.kadaken.wayfinder.picker.desktop"
subst "$here/org.freedesktop.FileManager1.service" "$dbus/org.freedesktop.FileManager1.service"

for pair in $icon_sizes; do
  size="${pair%%:*}"; file="${pair#*:}"
  run install -d "$icons/$size/apps"
  run install -m 0644 "$root/src-tauri/icons/$file" "$icons/$size/apps/com.kadaken.wayfinder.png"
done

if [ "$with_portal" = 1 ]; then
  run install -d "$portals"
  subst "$here/org.freedesktop.impl.portal.desktop.wayfinder.service" \
        "$dbus/org.freedesktop.impl.portal.desktop.wayfinder.service"
  if [ "$dry" = 1 ]; then
    printf '  would: write %s\n' "$portals/wayfinder.portal"
  else
    install -m 0644 "$here/wayfinder.portal" "$portals/wayfinder.portal"
  fi

  if [ -z "$destdir" ]; then
    # Tell xdg-desktop-portal that Wayfinder is the file dialog. Anything
    # already in the file is kept, and the old file is saved as .bak.
    key='org.freedesktop.impl.portal.FileChooser'
    if [ "$dry" = 1 ]; then
      printf '  would: set %s=wayfinder in %s\n' "$key" "$portals_conf"
    elif [ -f "$portals_conf" ] \
         && grep -q "^$key[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$" "$portals_conf"; then
      say "  $portals_conf already names Wayfinder as the file dialog"
    elif [ -f "$portals_conf" ]; then
      cp -p "$portals_conf" "$portals_conf.bak"
      if grep -q "^$key[[:space:]]*=" "$portals_conf.bak"; then
        # A file dialog is already named: point that same line at Wayfinder.
        sed "s|^$key[[:space:]]*=.*|$key=wayfinder|" "$portals_conf.bak" > "$portals_conf"
      elif grep -q '^\[preferred\]' "$portals_conf.bak"; then
        awk -v key="$key" '{ print } /^\[preferred\]/ && !done { print key"=wayfinder"; done=1 }' \
          "$portals_conf.bak" > "$portals_conf"
      else
        cat "$portals_conf.bak" > "$portals_conf"
        printf '[preferred]\n%s=wayfinder\n' "$key" >> "$portals_conf"
      fi
      say "  set $key=wayfinder in $portals_conf (previous file kept as $portals_conf.bak)"
    else
      install -d "$(dirname "$portals_conf")"
      printf '[preferred]\ndefault=*\n%s=wayfinder\n' "$key" > "$portals_conf"
      say "  wrote $portals_conf"
    fi
  fi
else
  say "  skipping the file-dialog backend (--no-portal)"
fi

refresh_caches

if [ -z "$destdir" ]; then
  if command -v xdg-mime >/dev/null 2>&1 && [ "$dry" != 1 ]; then
    xdg-mime default com.kadaken.wayfinder.desktop inode/directory >/dev/null 2>&1 || true
  fi
  restart_portal
fi

say ""
say "Installed:"
installed_files | while IFS= read -r f; do [ -e "$f" ] || [ -L "$f" ] || continue; say "  $f"; done

case ":${PATH}:" in
  *":$bindir:"*) ;;
  *) say ""; say "Note: $bindir is not in your PATH. Add it, or run $bindir/wayfinder directly." ;;
esac

say ""
say "Run 'wayfinder' to open it. Programs asking for a file will use Wayfinder"
say "once the portal has restarted; log out and back in if an app still shows"
say "the old dialog. Remove everything again with: $0 --uninstall"
