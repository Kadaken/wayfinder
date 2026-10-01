#!/usr/bin/env bash
# Private headless GTK test gate. One unique run root under ~/Projects/Scratch,
# removed by THIS parent process on exit, interrupt, terminate, abort or timeout.
#   headless-gtk-test.sh full        the full gtk_headless_suite
#   headless-gtk-test.sh d154        the standalone real-card gate
#   headless-gtk-test.sh d53         the standalone keyboard-focus-binding gate
#   headless-gtk-test.sh d155        the standalone transfer progress/cancel gate
#   headless-gtk-test.sh d185        the standalone forced-cross-device transfer gate
#   headless-gtk-test.sh d186        the standalone real loopback SMB gate (own netns)
#   headless-gtk-test.sh d188        the standalone read-only-zip-folder gate
#   headless-gtk-test.sh d187        the carried round-187 undo/redo regression selector
#   headless-gtk-test.sh -- CMD...   any command (used by the self-check)
# Env: WF_HEADLESS_TIMEOUT seconds (default 900).
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$HOME/Projects/Scratch"
mkdir -p "$scratch"
root="$(mktemp -d "$scratch/wf-headless-XXXXXX")" || exit 97
case "$root" in "$scratch"/wf-headless-??????) ;; *) echo "bad run root" >&2; exit 97;; esac
echo "RUN_ROOT=$root" >&2
child=""
mode=""

reap_root_processes() {
  # Backstop for the case the in-test RAII guard never ran at all (a hard
  # SIGKILL on timeout skips Drop): find every process anywhere on the host
  # whose argv names a path under this run's own root (smbd, samba-dcerpcd,
  # rpcd_*, dbus-daemon, ...) and kill it directly, independent of whatever
  # process group it ended up in.
  local pids pid
  for _ in 1 2; do
    pids="$(pgrep -f -- "$root" 2>/dev/null)"
    [ -z "$pids" ] && return 0
    for pid in $pids; do kill -TERM "$pid" 2>/dev/null; done
    sleep 0.5
  done
  pids="$(pgrep -f -- "$root" 2>/dev/null)"
  for pid in $pids; do kill -KILL "$pid" 2>/dev/null; done
  sleep 0.2
  pids="$(pgrep -f -- "$root" 2>/dev/null)"
  if [ -n "$pids" ]; then
    echo "D186 cleanup: process(es) still naming $root after TERM+KILL: $pids" >&2
  fi
}

cleanup() {
  trap - EXIT INT TERM
  if [ -n "$child" ]; then
    kill -TERM -- "-$child" 2>/dev/null
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 -- "-$child" 2>/dev/null || break; sleep 0.5; done
    kill -KILL -- "-$child" 2>/dev/null
  fi
  # Not just a d186 concern: any leaked descendant naming this run's root
  # in its own argv must die before the root is removed, on every mode.
  reap_root_processes
  case "$root" in "$scratch"/wf-headless-??????) rm -rf -- "$root";; esac
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

export WF_TEST_SCRATCH_ROOT="$root"
export GDK_BACKEND=x11 GTK_A11Y=none G_DEBUG=fatal-criticals WAYFINDER_HEADLESS_GTK_TEST=1
unset WAYLAND_DISPLAY
t="${WF_HEADLESS_TIMEOUT:-900}"
mode="${1:-full}"
cargo_t=(cargo test --quiet --features guide --manifest-path "$here/src-tauri/Cargo.toml")
case "$mode" in
  full) cmd=("${cargo_t[@]}" gtk_headless_suite);;
  d154) export WAYFINDER_D154_ONLY=1
        cmd=("${cargo_t[@]}" guide_sitting_attached_card_d154_alone -- --nocapture);;
  d53)  export WAYFINDER_D53_ONLY=1
        cmd=("${cargo_t[@]}" d53_keyboard_focus_binds_to_intended_surface_alone -- --nocapture);;
  d155) export WAYFINDER_D155_ONLY=1
        cmd=("${cargo_t[@]}" transfer_progress_and_cancel_d155_alone -- --nocapture);;
  d185) export WAYFINDER_D185_ONLY=1
        cmd=("${cargo_t[@]}" transfer_progress_and_cancel_d185_alone -- --nocapture);;
  d188) export WAYFINDER_D188_ONLY=1
        cmd=("${cargo_t[@]}" zip_read_only_browse_open_copy_out_d188_alone -- --nocapture);;
  d187) export WAYFINDER_D187_ONLY=1
        cmd=("${cargo_t[@]}" undo_redo_d187_regression_alone -- --nocapture);;
  d186) export WAYFINDER_D186_ONLY=1
        subgid="$(awk -F: -v u="$(id -un)" '$1==u{print $2":"$3; found=1} END{if(!found) exit 1}' /etc/subgid)" \
          || { echo "no /etc/subgid range for $(id -un): required for the d186 loopback namespace" >&2; exit 97; }
        # dbus-run-session's own bus socket/state must live under this run's
        # root too, never /tmp: give it a private XDG_RUNTIME_DIR before it
        # starts (the Rust fixture later points its own XDG_RUNTIME_DIR at
        # its own sub-root for the same reason).
        xdg_outer="$root/xdg-outer"
        mkdir -m 0700 -p "$xdg_outer"
        export XDG_RUNTIME_DIR="$xdg_outer"
        inner_test=("${cargo_t[@]}" smb_real_loopback_d186_alone -- --nocapture)
        cmd=(unshare --map-root-user --map-groups "1:${subgid}" --net --mount -- \
             bash -c 'set -e; ip link set lo up; exec dbus-run-session -- "$@"' bash "${inner_test[@]}")
        host_mounts_before="$(cat /proc/self/mountinfo)";;
  --) shift; cmd=("$@");;
  *) echo "usage: $0 full|d154|d53|d155|d185|d186|d188|d187|-- CMD..." >&2; exit 2;;
esac

setsid timeout -k 5 "$t" xvfb-run -a -s "-screen 0 1920x1080x24" "${cmd[@]}" &
child=$!
wait "$child"
st=$?
child=""
if [ "$mode" = d186 ]; then
  host_mounts_after="$(cat /proc/self/mountinfo)"
  if [ "$host_mounts_before" != "$host_mounts_after" ]; then
    echo "HOST MOUNT TABLE CHANGED across the d186 run — this must never happen" >&2
    diff <(printf '%s' "$host_mounts_before") <(printf '%s' "$host_mounts_after") >&2 || true
    st=97
  else
    echo "host mount table unchanged across the d186 run" >&2
  fi
fi
exit "$st"
