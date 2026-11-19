#!/usr/bin/env bash
# What Wayfinder needs on the machine besides its own two binaries, how to
# find out whether it is there, and how to put it there.
#
# Sourced by packaging/install.sh. Can also be run on its own:
#
#   ./packaging/dependencies.sh            say what is missing, change nothing
#   ./packaging/dependencies.sh --install  install what is missing
#
# Why this file exists: Wayfinder draws picture previews, video posters and
# network shares by asking other programs on the machine to do the decoding.
# When one of those is absent the app still runs, but a whole feature quietly
# does nothing and never says why. AVIF pictures showed no preview on a
# machine with no AVIF decoder and gave no reason at all (7 Nov 2026).
# Nothing here is guessed at install time: every item is checked by running
# the thing, not by asking the package manager what it thinks it installed.
#
# SPDX-License-Identifier: GPL-3.0-or-later

# ---------------------------------------------------------------- the list
#
# One line per thing Wayfinder leans on:
#   key | what it gives you | what you lose without it | how it is checked
#
# `wf_dep_check_<key>` returns 0 when the thing works.

WF_DEPS="ffmpeg gstreamer gstav pixbuf gvfs portal sevenzip"

wf_dep_title() {
  case "$1" in
    ffmpeg)    echo "AVIF, HEIC and JPEG-XL picture previews" ;;
    gstreamer) echo "video posters in Grid view" ;;
    gstav)     echo "video posters for MP4, MKV and MOV" ;;
    pixbuf)    echo "PNG, JPEG, WebP and GIF previews" ;;
    gvfs)      echo "Windows and Samba network shares" ;;
    portal)    echo "Wayfinder as every program's Open and Save box" ;;
    sevenzip)  echo "Extract Here for zip, 7z, rar and other archives" ;;
  esac
}

wf_dep_cost() {
  case "$1" in
    ffmpeg)    echo "those pictures show a plain file icon and never a preview" ;;
    gstreamer) echo "every video shows a plain film icon" ;;
    gstav)     echo "most videos show a plain film icon" ;;
    pixbuf)    echo "no picture previews at all" ;;
    gvfs)      echo "Connect to Server cannot reach a share" ;;
    portal)    echo "other programs keep their own file dialog" ;;
    sevenzip)  echo "Extract Here fails on every archive that is not a tarball" ;;
  esac
}

# D-280: gst-inspect-1.0 builds GStreamer's plugin registry as a side effect,
# so probing for codecs wrote ~/.cache/gstreamer-1.0 into a home directory the
# caller was only reading. Probe against a throwaway registry instead, so
# --check and --dry-run leave nothing behind.
wf_gst_inspect() {
  _probe_cache="${TMPDIR:-/tmp}/wayfinder-probe-cache.$$"
  mkdir -p "$_probe_cache" 2>/dev/null || true
  # XDG_CACHE_HOME covers every cache a loaded plugin writes, not only
  # GStreamer's registry: a hardware video plugin pulls in the GPU driver,
  # which writes its own shader caches.
  XDG_CACHE_HOME="$_probe_cache" \
  GST_REGISTRY="$_probe_cache/gst-registry.bin" \
  MESA_SHADER_CACHE_DISABLE=true \
    gst-inspect-1.0 "$1" >/dev/null 2>&1
  rc=$?
  rm -rf "$_probe_cache"
  return $rc
}

wf_dep_ok() {
  case "$1" in
    ffmpeg)
      command -v ffmpeg >/dev/null 2>&1
      ;;
    gstreamer)
      command -v gst-inspect-1.0 >/dev/null 2>&1 \
        && wf_gst_inspect playbin
      ;;
    gstav)
      command -v gst-inspect-1.0 >/dev/null 2>&1 \
        && wf_gst_inspect libav
      ;;
    pixbuf)
      # gdk-pixbuf is a hard requirement of the binary itself, so if the app
      # links it at all this passes. Checked anyway, because a stripped
      # container can have the library and no loaders.
      [ -n "$(ls -A /usr/lib64/gdk-pixbuf-2.0/*/loaders 2>/dev/null \
              || ls -A /usr/lib/*/gdk-pixbuf-2.0/*/loaders 2>/dev/null)" ] \
        || command -v gdk-pixbuf-query-loaders >/dev/null 2>&1
      ;;
    gvfs)
      command -v gio >/dev/null 2>&1 && gio mount --list --schemes 2>/dev/null | grep -q smb
      ;;
    portal)
      [ -n "$(ls -A /usr/share/xdg-desktop-portal/portals 2>/dev/null)" ]
      ;;
    sevenzip)
      command -v 7z >/dev/null 2>&1
      ;;
  esac
}

# ------------------------------------------------------- distro and packages
wf_distro() {
  local id="" like=""
  if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    id="$(. /etc/os-release && printf '%s' "${ID:-}")"
    like="$(. /etc/os-release && printf '%s' "${ID_LIKE:-}")"
  fi
  case " $id $like " in
    *" fedora "*|*" rhel "*|*" centos "*) echo fedora ;;
    *" debian "*|*" ubuntu "*)            echo debian ;;
    *" arch "*)                           echo arch ;;
    *" suse "*|*" opensuse "*)            echo suse ;;
    *)                                    echo unknown ;;
  esac
}

# The packages that supply each item, per distro family.
wf_dep_packages() {
  local key="$1" distro="$2"
  case "$distro:$key" in
    fedora:ffmpeg)    echo "ffmpeg-free libavif libheif" ;;
    fedora:gstreamer) echo "gstreamer1-plugins-base gstreamer1-plugins-good" ;;
    fedora:gstav)     echo "gstreamer1-plugins-bad-free gstreamer1-plugins-bad-free-fluidsynth gstreamer1-plugin-libav" ;;
    fedora:pixbuf)    echo "gdk-pixbuf2 glycin-loaders" ;;
    fedora:gvfs)      echo "gvfs-smb" ;;
    fedora:portal)    echo "xdg-desktop-portal" ;;
    fedora:sevenzip)    echo "7zip" ;;

    debian:ffmpeg)    echo "ffmpeg libavif-bin libheif1" ;;
    debian:gstreamer) echo "gstreamer1.0-plugins-base gstreamer1.0-plugins-good" ;;
    debian:gstav)     echo "gstreamer1.0-plugins-bad gstreamer1.0-libav" ;;
    debian:pixbuf)    echo "libgdk-pixbuf-2.0-0 glycin-loaders" ;;
    debian:gvfs)      echo "gvfs-backends" ;;
    debian:portal)    echo "xdg-desktop-portal" ;;
    debian:sevenzip)    echo "p7zip-full" ;;

    arch:ffmpeg)      echo "ffmpeg libavif libheif" ;;
    arch:gstreamer)   echo "gst-plugins-base gst-plugins-good" ;;
    arch:gstav)       echo "gst-plugins-bad gst-libav" ;;
    arch:pixbuf)      echo "gdk-pixbuf2 glycin" ;;
    arch:gvfs)        echo "gvfs-smb" ;;
    arch:portal)      echo "xdg-desktop-portal" ;;
    arch:sevenzip)    echo "7zip" ;;

    suse:ffmpeg)      echo "ffmpeg libavif16 libheif1" ;;
    suse:gstreamer)   echo "gstreamer-plugins-base gstreamer-plugins-good" ;;
    suse:gstav)       echo "gstreamer-plugins-bad gstreamer-plugins-libav" ;;
    suse:pixbuf)      echo "gdk-pixbuf glycin-loaders" ;;
    suse:gvfs)        echo "gvfs-backend-samba" ;;
    suse:portal)      echo "xdg-desktop-portal" ;;
    suse:sevenzip)    echo "7zip" ;;

    *) echo "" ;;
  esac
}

wf_install_command() {
  case "$1" in
    fedora) echo "sudo dnf install -y" ;;
    debian) echo "sudo apt-get install -y" ;;
    arch)   echo "sudo pacman -S --needed --noconfirm" ;;
    suse)   echo "sudo zypper install -y" ;;
    *)      echo "" ;;
  esac
}

# ------------------------------------------------------------- the report
#
# Prints the state of every item. Sets WF_DEPS_MISSING to the keys that are
# not there. Returns 0 when nothing is missing.
wf_dep_report() {
  local key
  WF_DEPS_MISSING=""
  printf '\n'
  printf 'What Wayfinder uses on this machine\n'
  printf -- '-----------------------------------\n'
  for key in $WF_DEPS; do
    if wf_dep_ok "$key"; then
      printf '  [ok]      %s\n' "$(wf_dep_title "$key")"
    else
      printf '  [MISSING] %s\n' "$(wf_dep_title "$key")"
      printf '            without it, %s\n' "$(wf_dep_cost "$key")"
      WF_DEPS_MISSING="$WF_DEPS_MISSING $key"
    fi
  done
  WF_DEPS_MISSING="${WF_DEPS_MISSING# }"
  [ -z "$WF_DEPS_MISSING" ]
}

# What to say once the report is printed. $1 is 1 when the installer has been
# told it may install the missing pieces itself.
wf_dep_advise() {
  local may_install="${1:-0}" distro packages key all cmd
  if [ -z "${WF_DEPS_MISSING:-}" ]; then
    printf '\nEverything Wayfinder needs is already here. Previews, video\n'
    printf 'posters and network shares will all work.\n'
    return 0
  fi
  distro="$(wf_distro)"
  cmd="$(wf_install_command "$distro")"
  all=""
  for key in $WF_DEPS_MISSING; do
    packages="$(wf_dep_packages "$key" "$distro")"
    all="$all $packages"
  done
  # Squeeze repeats and spaces.
  all="$(printf '%s\n' $all | sort -u | tr '\n' ' ')"
  all="${all% }"

  if [ -z "$cmd" ] || [ -z "$all" ]; then
    printf '\nThis is not a distribution the installer knows how to name packages\n'
    printf 'for. Install the pieces listed as MISSING with your own package\n'
    printf 'manager; ffmpeg, the GStreamer good/bad/libav plugins and gvfs are\n'
    printf 'what they are called nearly everywhere.\n'
    return 1
  fi

  if [ "$may_install" = 1 ]; then
    printf '\nInstalling the missing pieces:\n  %s %s\n\n' "$cmd" "$all"
    # shellcheck disable=SC2086
    $cmd $all
    return $?
  fi

  printf '\nWayfinder will run, but those features will do nothing. One command\n'
  printf 'fixes all of them:\n\n  %s %s\n\n' "$cmd" "$all"
  printf 'Or run the installer again with --with-codecs and it will do it.\n'
  return 1
}

# Running this file on its own.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  set -uo pipefail
  if [ "${1:-}" = "--install" ]; then
    wf_dep_report || true
    wf_dep_advise 1
  else
    wf_dep_report || true
    wf_dep_advise 0
  fi
fi
