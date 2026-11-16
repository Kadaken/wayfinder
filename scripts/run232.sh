#!/usr/bin/env bash
# The private-HOME runner the clipboard tests (D-232) demand, and the only way
# to run the headless GTK suite: those tests refuse to start unless HOME, every
# XDG directory and the session bus live inside one throwaway tree under
# ~/Projects/Scratch, so a test can never touch the real desktop's clipboard,
# config or bus. The tree, and the bus started in it, are removed on exit.
#   run232.sh                 the full gtk_headless_suite
#   run232.sh <mode>          any headless-gtk-test.sh mode (d226, d154, ...)
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$HOME/Projects/Scratch"
mkdir -p "$scratch"
home="$(mktemp -d "$scratch/wf-home232-XXXXXX")" || exit 97
case "$home" in "$scratch"/wf-home232-??????) ;; *) echo "bad private home" >&2; exit 97;; esac
echo "PRIVATE_HOME=$home" >&2
bus_pid=""
cleanup() {
  trap - EXIT INT TERM
  [ -n "$bus_pid" ] && kill -TERM "$bus_pid" 2>/dev/null
  case "$home" in "$scratch"/wf-home232-??????) rm -rf -- "$home";; esac
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for d in data config cache state run; do mkdir -m 0700 -p "$home/$d"; done
# A real, empty trash for this HOME: the trash tests read and write it, and
# without it they fail on a missing folder rather than on their own subject.
# gio reads XDG_DATA_HOME for the Trash; the app computes $HOME/.local/share
# itself. Point XDG_DATA_HOME at that same place, or a trashed file lands in
# one tree and is looked for in the other.
mkdir -m 0700 -p "$home/.local/share/Trash/files" "$home/.local/share/Trash/info"
mkdir -p "$home/Desktop" "$home/Documents" "$home/Downloads"
# The toolchain still has to be found after HOME moves: rustup and cargo keep
# their install under the real home, and reading them touches nothing a test
# is being kept away from (the clipboard, the config, the bus).
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$home"
export XDG_DATA_HOME="$home/.local/share" XDG_CONFIG_HOME="$home/config"
export XDG_CACHE_HOME="$home/cache" XDG_STATE_HOME="$home/state"
export XDG_RUNTIME_DIR="$home/run"

# A session bus of its own, with its socket inside the private HOME, and no
# service directory at all.
#
# D-273. A private bus is not enough on its own, and neither is pointing the
# daemon at an empty XDG_DATA_DIRS: `<standard_session_servicedirs/>` in
# /usr/share/dbus-1/session.conf always adds the built-in
# /usr/share/dbus-1/services on top of whatever the environment says. That
# folder holds org.freedesktop.portal.Desktop.service, so the first test that
# asked for a file dialog had this bus start a real xdg-desktop-portal
# carrying the throwaway HOME. It cannot read the user database from there and
# aborts inside the GNOME proxy resolver -- 33 of those core dumps on 16
# November alone -- and the desktop loses its file dialogs with it. A config
# file of our own, naming one empty service folder and nothing else, is the
# only thing that leaves the portal unknown on this bus.
mkdir -m 0700 -p "$home/no-services"
command cat > "$home/session.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$home/bus</listen>
  <servicedir>$home/no-services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
XML
bus_pid="$(env HOME="$home" \
  XDG_DATA_HOME="$home/no-services" XDG_DATA_DIRS="$home/no-services" \
  dbus-daemon --config-file="$home/session.conf" \
  --address="unix:path=$home/bus" --fork --print-pid)" || exit 97
export DBUS_SESSION_BUS_ADDRESS="unix:path=$home/bus"

# Checked on every run, before a single test starts: this bus must not be able
# to reach the desktop portal. If it ever can again, stop here rather than risk
# the desktop a second time.
if timeout 20 gdbus call --session --dest org.freedesktop.portal.Desktop \
     --object-path /org/freedesktop/portal/desktop \
     --method org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
  echo "run232: this bus can start the desktop portal; refusing to run (D-273)" >&2
  exit 97
fi

if [ "$#" -eq 0 ]; then
  exec "$here/scripts/headless-gtk-test.sh" full
fi
# Every argument, not just the first: the `--` mode takes a whole command
# after it, and dropping the rest left xvfb-run printing its own usage.
exec "$here/scripts/headless-gtk-test.sh" "$@"
