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
#   --kde-apps      also send KDE/Qt apps through the portal, so they use
#                   Wayfinder too. Off by default: it makes Qt apps take their
#                   theming from the portal as well, and some people would
#                   rather keep KDE's own file dialog. Takes effect at the next
#                   login. Undone by --uninstall, or by --no-kde-apps.
#   --no-kde-apps   remove that setting and give KDE apps their own dialog back
#   --with-codecs   install the codecs and helpers Wayfinder needs for picture
#                   previews, video posters and network shares. Without this
#                   the installer only says what is missing and leaves it.
#   --check         only say what is on the machine and what is missing, then
#                   stop. Installs nothing.
#   --no-build      stop instead of building when the binaries are missing
#   --dry-run       print every step, change nothing
#
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Kadaken

set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$here/.." && pwd)"

prefix=""
destdir=""
system=0
with_portal=1
kde_apps=
build=1
dry=0
uninstall=0
with_codecs=0
check_only=0

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
    --kde-apps)   kde_apps=1 ;;
    --no-kde-apps) kde_apps=0 ;;
    --with-codecs) with_codecs=1 ;;
    --check)      check_only=1 ;;
    --no-build)   build=0 ;;
    --dry-run)    dry=1 ;;
    --uninstall)  uninstall=1 ;;
    -h|--help)    sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)            die "unknown option: $1" ;;
  esac
  shift
done

if [ -z "$prefix" ]; then
  if [ "$system" = 1 ]; then prefix="/usr/local"; else prefix="${HOME}/.local"; fi
fi

# What Wayfinder leans on besides its own binaries: codecs, decoders and the
# portal. Kept in its own file so it can be run on its own as well.
# shellcheck source=packaging/dependencies.sh
. "$here/dependencies.sh"

if [ "$check_only" = 1 ]; then
  wf_dep_report || true
  wf_dep_advise "$with_codecs" || true
  exit 0
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
# KDE/Qt apps do not ask the portal at all unless Qt is told to use it. This
# file is the switch, and it is written only when the user asks for it.
qt_env_conf="$conf_home/environment.d/95-wayfinder-qt.conf"
qt_env_marker="# written by the Wayfinder installer (--kde-apps)"

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
  [ -n "${WAYFINDER_INSTALL_NO_RESTART:-}" ] && return 0
  say "Restarting the desktop portal so the change takes effect now."
  # The backend first. D-Bus started it on demand and keeps it running, so
  # after an upgrade it is still serving file dialogs from the binary this
  # installer just replaced - the user gets the old build until they log out.
  # Stopping it is safe: D-Bus starts the new one the next time an app asks
  # for a dialog.
  # D-288: never match on the process name. Linux truncates comm to 15
  # characters and "wayfinder-portal" is 16, so `pkill -x wayfinder-portal`
  # can never match anything and the old backend kept running. Match the
  # full path of the binary this installer owns, and say what happened.
  portal_bin="$prefix/bin/wayfinder-portal"
  if command -v pkill >/dev/null 2>&1; then
    if [ "$dry" = 1 ]; then
      printf '  would: stop any running %s\n' "$portal_bin"
    elif pkill -f -- "^${portal_bin}( |$)"; then
      say "  stopped the running portal backend; D-Bus will start the new one."
    else
      say "  no portal backend was running."
    fi
  fi
  if command -v systemctl >/dev/null 2>&1; then
    run systemctl --user try-restart xdg-desktop-portal.service || true
  fi
}

# Writes or removes the one line that sends KDE/Qt apps through the portal.
# Only a file this installer wrote is ever removed.
write_qt_env() {
  if [ "$dry" = 1 ]; then
    printf '  would: write %s\n' "$qt_env_conf"
    return 0
  fi
  mkdir -p "$(dirname "$qt_env_conf")"
  printf '%s\nQT_QPA_PLATFORMTHEME=xdgdesktopportal\n' "$qt_env_marker" > "$qt_env_conf"
  say "  wrote $qt_env_conf -- KDE apps use Wayfinder from your next login"
}

remove_qt_env() {
  [ -f "$qt_env_conf" ] || return 0
  if ! head -n1 "$qt_env_conf" | grep -qF -- "$qt_env_marker"; then
    say "  leaving $qt_env_conf alone: this installer did not write it"
    return 0
  fi
  if [ "$dry" = 1 ]; then
    printf '  would: remove %s\n' "$qt_env_conf"
    return 0
  fi
  rm -f "$qt_env_conf"
  say "  removed $qt_env_conf -- KDE apps get their own dialog back at next login"
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

  remove_qt_env
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
    # Built through the remap flags, so the binaries never carry the path
    # they were built from. scripts/release-rustflags.sh says why.
    if [ -x "$root/scripts/release-rustflags.sh" ]; then
      CARGO_ENCODED_RUSTFLAGS="$("$root/scripts/release-rustflags.sh")" \
        cargo build --release --manifest-path "$root/src-tauri/Cargo.toml"
    else
      cargo build --release --manifest-path "$root/src-tauri/Cargo.toml"
    fi
  fi
fi

# Announced before anything is copied, so nobody finds out weeks later that
# previews were silently doing nothing (7 Nov 2026, the owner: "it should be
# announced upon install").
wf_dep_report || true
if [ -n "${WF_DEPS_MISSING:-}" ] && [ "$with_codecs" = 1 ] && [ "$dry" != 1 ] && [ -z "$destdir" ]; then
  wf_dep_advise 1 || say "Could not install everything. Wayfinder is still installed below."
  say ""
  say "Checking again:"
  wf_dep_report || true
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

# KDE/Qt apps: only ever on request, either way.
if [ -z "$destdir" ]; then
  case "$kde_apps" in
    1) write_qt_env ;;
    0) remove_qt_env ;;
  esac
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

if [ -n "${WF_DEPS_MISSING:-}" ]; then
  say ""
  say "Before you finish: the features marked MISSING above will do nothing"
  say "until those packages are on the machine."
  wf_dep_advise 0 || true
else
  say ""
  say "Every codec and helper Wayfinder needs is present: picture previews,"
  say "video posters and network shares will all work."
fi

say ""
say "Run 'wayfinder' to open it. Programs asking for a file will use Wayfinder"
say "once the portal has restarted; log out and back in if an app still shows"
say "the old dialog. Remove everything again with: $0 --uninstall"

if [ "$kde_apps" != 1 ] && [ "${XDG_CURRENT_DESKTOP:-}" = "KDE" ]; then
  say ""
  say "KDE apps (KWrite, Dolphin, Gwenview) draw their own file dialog and never"
  say "ask the portal, so they will not use Wayfinder. Run this installer again"
  say "with --kde-apps to send them through it; Qt apps then take their theming"
  say "from the portal as well. --no-kde-apps undoes it."
fi
