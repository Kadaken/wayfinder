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
#   --no-portal     install the file manager only and remove an earlier Wayfinder
#                   file-dialog backend from this prefix
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
if [ "$with_portal" = 0 ] && [ "$uninstall" = 0 ]; then
  WF_DEPS=" $WF_DEPS "
  WF_DEPS="${WF_DEPS/ portal / }"
fi

if [ "$check_only" = 1 ]; then
  wf_dep_report || true
  # D-426: --check reports the install command but never runs it.
  wf_dep_advise 0 || true
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
portal_bin="$bindir/wayfinder-portal"
portal_service="$dbus/org.freedesktop.impl.portal.desktop.wayfinder.service"
portal_removed=0
icons="$d$datadir/icons/hicolor"

conf_home="${XDG_CONFIG_HOME:-$HOME/.config}"
portals_conf="$conf_home/xdg-desktop-portal/portals.conf"
portal_prev_choice="${XDG_DATA_HOME:-$HOME/.local/share}/wayfinder/previous-portal-filechooser"
# KDE/Qt apps do not ask the portal at all unless Qt is told to use it. This
# file is the switch, and it is written only when the user asks for it.
qt_env_conf="$conf_home/environment.d/95-wayfinder-qt.conf"
qt_env_marker="# written by the Wayfinder installer (--kde-apps)"
# The folder app that was the default before Wayfinder took it, so --uninstall
# can hand folders back instead of leaving a default that no longer exists.
prev_folder_app="${XDG_DATA_HOME:-$HOME/.local/share}/wayfinder/previous-folder-app"
wf_desktop=com.kadaken.wayfinder.desktop
mimeapps="$conf_home/mimeapps.list"

icon_sizes="32x32:32x32.png 64x64:64x64.png 128x128:128x128.png 256x256:128x128@2x.png 512x512:icon.png"

file_manager_files() {
  printf '%s\n' \
    "$d$bindir/wayfinder" \
    "$d$bindir/wayfinder-native" \
    "$d$bindir/wayfinder-new-window" \
    "$apps/com.kadaken.wayfinder.desktop" \
    "$dbus/org.freedesktop.FileManager1.service"
  local pair
  for pair in $icon_sizes; do
    printf '%s\n' "$icons/${pair%%:*}/apps/com.kadaken.wayfinder.png"
  done
}

portal_files() {
  printf '%s\n' \
    "$d$portal_bin" \
    "$apps/com.kadaken.wayfinder.picker.desktop" \
    "$portal_service" \
    "$portals/wayfinder.portal"
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
  # Stop the backend first. D-Bus can keep it running after the installer
  # replaces or removes its binary.
  # D-288: never match on the process name. Linux truncates comm to 15
  # characters and "wayfinder-portal" is 16, so `pkill -x wayfinder-portal`
  # can never match anything and the old backend kept running. Match the
  # full path of the binary this installer owns, and say what happened.
  if command -v pkill >/dev/null 2>&1; then
    if [ "$dry" = 1 ]; then
      printf '  would: stop any running %s\n' "$portal_bin"
    elif pkill -f -- "^${portal_bin}( |$)"; then
      say "  stopped the running portal backend."
    else
      say "  no portal backend was running."
    fi
  fi
  # S7 @ 0.9.39: with no session bus (a container, ssh, a TTY) systemctl prints
  # a raw "Failed to connect to user scope bus"; say it plainly instead.
  if command -v systemctl >/dev/null 2>&1; then
    if [ "$dry" = 1 ]; then
      printf '  would: systemctl --user try-restart xdg-desktop-portal.service\n'
    elif systemctl --user try-restart xdg-desktop-portal.service 2>/dev/null; then
      say "  the portal restarted."
    else
      say "  could not reach your desktop session to restart the portal;"
      say "  it picks up the change at your next login."
    fi
  fi
}

# Writes or removes the one line that sends KDE/Qt apps through the portal.
# Only a file this installer wrote is ever removed.
write_qt_env() {
  # S14 0.9.39: a file of the same name that this installer did not write is
  # the user's own; it is never overwritten, just as remove_qt_env never removes it.
  if [ -e "$qt_env_conf" ] && ! head -n1 "$qt_env_conf" 2>/dev/null | grep -qF -- "$qt_env_marker"; then
    say "  leaving $qt_env_conf alone: this installer did not write it"
    return 0
  fi
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

# Install: remember the folder app Wayfinder replaces, then take folders.
# A reinstall never overwrites the record with Wayfinder itself.
take_folder_app() {
  command -v xdg-mime >/dev/null 2>&1 || return 0
  local cur=""
  # Only the user's own choice is recorded, read from their mimeapps.list, so
  # uninstall restores exactly that file and never writes a system default in.
  [ -f "$mimeapps" ] && cur="$(awk '
    /^\[/ { indef = ($0 == "[Default Applications]") ; next }
    indef && index($0, "inode/directory=") == 1 { print substr($0, 17); exit }
  ' "$mimeapps")"
  cur="${cur%;}"
  # D-452: the dry run lists this step too; it writes the user's folder default.
  # W5.4 0.9.39: it names the record only when the real run would write one.
  if [ "$dry" = 1 ]; then
    if [ -n "$cur" ] && [ "$cur" != "$wf_desktop" ]; then
      printf '  would: record the current folder app (%s) in %s\n' "$cur" "$prev_folder_app"
    fi
    printf '  would: xdg-mime default %s inode/directory\n' "$wf_desktop"
    return 0
  fi
  if [ -n "$cur" ] && [ "$cur" != "$wf_desktop" ]; then
    mkdir -p "$(dirname "$prev_folder_app")"
    printf '%s\n' "$cur" > "$prev_folder_app"
  fi
  mkdir -p "$conf_home"
  # D-431: xdg-mime needs the user's config directory to exist on a fresh account.
  if ! xdg-mime default "$wf_desktop" inode/directory >/dev/null 2>&1; then
    say "Folder default was not set."
  fi
}

# Uninstall: give folders back to the app recorded at install, or drop the
# Wayfinder default so the desktop's own default applies again. Left alone if
# the user already chose another app.
give_back_folder_app() {
  local prev=""
  # Read the file itself: `xdg-mime query` skips an entry whose desktop file is
  # gone, and the uninstall has just removed Wayfinder's.
  if [ -f "$mimeapps" ] && grep -q "^inode/directory=$wf_desktop;\?\$" "$mimeapps"; then
    [ -f "$prev_folder_app" ] && prev="$(head -n1 "$prev_folder_app")"
    if [ -n "$prev" ] && [ "$prev" != "$wf_desktop" ] && command -v xdg-mime >/dev/null 2>&1; then
      say "  giving folders back to $prev"
      run xdg-mime default "$prev" inode/directory
    elif [ -f "$mimeapps" ] && grep -q "^inode/directory=$wf_desktop;\?\$" "$mimeapps"; then
      say "  dropping the Wayfinder folder default from $mimeapps"
      if [ "$dry" = 1 ]; then
        printf '  would: edit %s\n' "$mimeapps"
      else
        # D-409g: ~/.config/mimeapps.list is often a symlink (dotfile
        # managers). Edit the file it points to; a plain mv would replace the
        # link with a regular file and cut it off from its source.
        local target
        target="$(readlink -f -- "$mimeapps")"
        [ -n "$target" ] || target="$mimeapps"
        { grep -v "^inode/directory=$wf_desktop;\?\$" "$target" || true; } > "$target.new"
        chmod --reference="$target" "$target.new" 2>/dev/null || true
        mv "$target.new" "$target"
      fi
    fi
  fi
  [ -f "$prev_folder_app" ] && run rm -f "$prev_folder_app"
  return 0
}

remove_portal() {
  local f previous_line
  while IFS= read -r f; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    say "  $f"
    run rm -f "$f"
    if [ "$dry" = 0 ]; then portal_removed=1; fi
  done < <(portal_files)

  [ -z "$destdir" ] || return 0
  if [ -f "$portals_conf" ] \
     && grep -q '^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$' "$portals_conf"; then
    say "  dropping the file-dialog line from $portals_conf"
    if [ "$dry" = 1 ]; then
      printf '  would: edit %s\n' "$portals_conf"
    else
      if [ -s "$portal_prev_choice" ]; then
        previous_line="$(head -n1 "$portal_prev_choice")"
        awk -v replacement="$previous_line" \
          '!done && $0 ~ /^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$/ { print replacement; done=1; next } { print }' \
          "$portals_conf" > "$portals_conf.new"
      else
        sed '/^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$/d' \
          "$portals_conf" > "$portals_conf.new"
      fi
      chmod --reference="$portals_conf" "$portals_conf.new" 2>/dev/null || true
      mv "$portals_conf.new" "$portals_conf"
      if [ -s "$portal_prev_choice" ]; then
        say "  file dialog choice put back: $previous_line"
      else
        say "  Wayfinder removed as the file dialog in $portals_conf"
      fi
      rm -f "$portal_prev_choice"
      portal_removed=1
    fi
  fi
  remove_qt_env
}

# ---------------------------------------------------------------- uninstall
if [ "$uninstall" = 1 ]; then
  say "Removing Wayfinder from $prefix"
  while IFS= read -r f; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    say "  $f"
    run rm -f "$f"
  done < <(file_manager_files)

  remove_portal
  [ -n "$destdir" ] || give_back_folder_app
  refresh_caches
  if [ "$portal_removed" = 1 ]; then restart_portal; fi
  say ""
  say "Uninstall keeps the following"
  say "  Settings folder: $conf_home/wayfinder"
  if [ -e "$portals_conf.bak" ] || [ -L "$portals_conf.bak" ]; then
    say "  Portal settings backup: $portals_conf.bak"
  fi
  say "  Any codec packages installed with --with-codecs"
  exit 0
fi

# ------------------------------------------------------------------ install
bin_src="$root/src-tauri/target/release"
if [ ! -x "$bin_src/wayfinder-native" ] || { [ "$with_portal" = 1 ] && [ ! -x "$bin_src/wayfinder-portal" ]; }; then
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
# previews were silently doing nothing (7 Nov 2026, Owner: "it should be
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
subst "$here/org.freedesktop.FileManager1.service" "$dbus/org.freedesktop.FileManager1.service"

for pair in $icon_sizes; do
  size="${pair%%:*}"; file="${pair#*:}"
  run install -d "$icons/$size/apps"
  run install -m 0644 "$root/src-tauri/icons/$file" "$icons/$size/apps/com.kadaken.wayfinder.png"
done

if [ "$with_portal" = 1 ]; then
  # The portal backend looks for wayfinder-native beside itself.
  run install -m 0755 "$bin_src/wayfinder-portal" "$d$portal_bin"
  subst "$here/com.kadaken.wayfinder.picker.desktop" "$apps/com.kadaken.wayfinder.picker.desktop"
  run install -d "$portals"
  subst "$here/org.freedesktop.impl.portal.desktop.wayfinder.service" \
        "$portal_service"
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
      # D-434: keep the original backup and record the exact replaced line for uninstall.
      if [ ! -e "$portals_conf.bak" ]; then
        cp -p "$portals_conf" "$portals_conf.bak"
      fi
      mkdir -p "$(dirname "$portal_prev_choice")"
      grep -m1 "^$key[[:space:]]*=" "$portals_conf" > "$portal_prev_choice" || : > "$portal_prev_choice"
      if grep -q "^$key[[:space:]]*=" "$portals_conf"; then
        # A file dialog is already named: point that same line at Wayfinder.
        sed "s|^$key[[:space:]]*=.*|$key=wayfinder|" "$portals_conf" > "$portals_conf.new"
        chmod --reference="$portals_conf" "$portals_conf.new" 2>/dev/null || true
        mv "$portals_conf.new" "$portals_conf"
      elif grep -q '^\[preferred\]' "$portals_conf"; then
        awk -v key="$key" '{ print } /^\[preferred\]/ && !done { print key"=wayfinder"; done=1 }' \
          "$portals_conf" > "$portals_conf.new"
        chmod --reference="$portals_conf" "$portals_conf.new" 2>/dev/null || true
        mv "$portals_conf.new" "$portals_conf"
      else
        cat "$portals_conf" > "$portals_conf.new"
        chmod --reference="$portals_conf" "$portals_conf.new" 2>/dev/null || true
        mv "$portals_conf.new" "$portals_conf"
        printf '[preferred]\n%s=wayfinder\n' "$key" >> "$portals_conf"
      fi
      say "  set $key=wayfinder in $portals_conf (previous file kept as $portals_conf.bak)"
    else
      install -d "$(dirname "$portals_conf")"
      mkdir -p "$(dirname "$portal_prev_choice")"
      : > "$portal_prev_choice"
      printf '[preferred]\ndefault=*\n%s=wayfinder\n' "$key" > "$portals_conf"
      say "  wrote $portals_conf"
    fi
  fi
# S13 0.9.39: a chooser line still naming Wayfinder counts as an earlier
# install even when every portal file is already gone; left alone it sends
# every program's file dialog to a backend that no longer exists.
elif [ -e "$d$portal_bin" ] || [ -L "$d$portal_bin" ] \
     || [ -e "$portal_service" ] || [ -L "$portal_service" ] \
     || { [ -z "$destdir" ] && [ -f "$portals_conf" ] \
          && grep -q '^org\.freedesktop\.impl\.portal\.FileChooser[[:space:]]*=[[:space:]]*wayfinder[[:space:]]*$' "$portals_conf"; }; then
  remove_portal
  if [ "$dry" = 1 ]; then
    say "  Would remove the earlier Wayfinder file dialog."
  else
    say "  Removed the earlier Wayfinder file dialog."
  fi
else
  say "  skipping the file-dialog backend (--no-portal)"
fi

# KDE/Qt apps: only ever on request, either way.
if [ -z "$destdir" ] && [ "$with_portal" = 1 ]; then
  case "$kde_apps" in
    1) write_qt_env ;;
    0) remove_qt_env ;;
  esac
fi

refresh_caches

if [ -z "$destdir" ]; then
  take_folder_app
  if [ "$with_portal" = 1 ] || [ "$portal_removed" = 1 ]; then
    restart_portal
  fi
fi

say ""
say "Installed:"
{
  file_manager_files
  if [ "$with_portal" = 1 ]; then portal_files; fi
} | while IFS= read -r f; do [ -e "$f" ] || [ -L "$f" ] || continue; say "  $f"; done

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
say "Run 'wayfinder' to open it."
if [ "$with_portal" = 1 ]; then
  say "Programs asking for a file will use Wayfinder once the portal has restarted."
  say "Log out and back in if an app still shows the old dialog."
fi
say "Run '$0 --uninstall' to remove Wayfinder."

if [ "$with_portal" = 1 ] && [ "$kde_apps" != 1 ] && [ "${XDG_CURRENT_DESKTOP:-}" = "KDE" ]; then
  say ""
  say "KDE apps (KWrite, Dolphin, Gwenview) draw their own file dialog and never"
  say "ask the portal, so they will not use Wayfinder. Run this installer again"
  say "with --kde-apps to send them through it; Qt apps then take their theming"
  say "from the portal as well. --no-kde-apps undoes it."
fi
