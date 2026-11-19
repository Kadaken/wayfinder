#!/bin/bash
# D-328 end to end: one running Wayfinder, then `--select` launched at it for a
# folder, a folder's parent, a file, a missing path and a plain folder. Each
# launch is forwarded to the running process and must leave one more window.
# Private session bus and a private X display; the owner's Wayfinder is never
# involved. Needs the release binary built first. Run it as:
#   xvfb-run -a -s "-screen 0 1920x1080x24" scripts/test-select-forwarded.sh
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${WF_BIN:-$here/src-tauri/target/release/wayfinder-native}"
[ -x "$BIN" ] || { echo "build $BIN first" >&2; exit 97; }
fail=0
scratch="$HOME/Projects/Scratch"; mkdir -p "$scratch"
home="$(mktemp -d "$scratch/wf-d328e2e-XXXXXX")" || exit 97
trap 'kill $bus_pid $p1 2>/dev/null; rm -rf -- "$home"' EXIT
for d in data config cache state run no-services; do mkdir -m 0700 -p "$home/$d"; done
mkdir -p "$home/lib/Images/Animals" "$home/lib/Music"; : > "$home/lib/Images/a.txt"
cat > "$home/session.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$home/bus</listen><servicedir>$home/no-services</servicedir>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
XML
bus_pid="$(env HOME="$home" XDG_DATA_HOME="$home/no-services" XDG_DATA_DIRS="$home/no-services" dbus-daemon --config-file="$home/session.conf" --address="unix:path=$home/bus" --fork --print-pid)" || exit 97
export HOME="$home" XDG_DATA_HOME="$home/data" XDG_CONFIG_HOME="$home/config" XDG_CACHE_HOME="$home/cache" XDG_STATE_HOME="$home/state" XDG_RUNTIME_DIR="$home/run"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$home/bus" GDK_BACKEND=x11 GTK_A11Y=none
unset WAYLAND_DISPLAY
wins() { xdotool search --onlyvisible --class "wayfinder" 2>/dev/null | wc -l; }
titles() { for w in $(xdotool search --onlyvisible --class "wayfinder" 2>/dev/null); do echo "   title: $(xdotool getwindowname $w)"; done; }
"$BIN" "$home/lib" >"$home/p1.log" 2>&1 & p1=$!
for i in $(seq 1 40); do [ "$(wins)" -ge 1 ] && break; sleep 0.5; done
echo "primary up, windows=$(wins)"; titles
run() { echo "--- $*"; n0=$(wins); timeout 20 "$BIN" "$@" >"$home/c.log" 2>&1; echo "client exit=$?"; for i in $(seq 1 20); do [ "$(wins)" -gt "$n0" ] && break; sleep 0.5; done; echo "windows $n0 -> $(wins)"; titles; [ "$(wins)" -gt "$n0" ] || { echo "FAIL: no window for: $*"; fail=1; }; }
run --select "$home/lib/Images"
run --select "$home/lib"
run --select "$home/lib/Images/a.txt"
run --select "$home/lib/Images/Gone/x.png"
run "$home/lib/Music"
[ "$fail" = 0 ] && echo "ALL FORWARDED LAUNCHES OPENED A WINDOW"
exit "$fail"
