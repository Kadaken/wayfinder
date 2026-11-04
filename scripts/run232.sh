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
mkdir -m 0700 -p "$home/data/Trash/files" "$home/data/Trash/info"
mkdir -p "$home/Desktop" "$home/Documents" "$home/Downloads"
# The toolchain still has to be found after HOME moves: rustup and cargo keep
# their install under the real home, and reading them touches nothing a test
# is being kept away from (the clipboard, the config, the bus).
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$home"
export XDG_DATA_HOME="$home/data" XDG_CONFIG_HOME="$home/config"
export XDG_CACHE_HOME="$home/cache" XDG_STATE_HOME="$home/state"
export XDG_RUNTIME_DIR="$home/run"

# A session bus of its own, with its socket inside the private HOME.
bus_pid="$(dbus-daemon --session --address="unix:path=$home/bus" --fork --print-pid)" || exit 97
export DBUS_SESSION_BUS_ADDRESS="unix:path=$home/bus"

exec "$here/scripts/headless-gtk-test.sh" "${1:-full}"
