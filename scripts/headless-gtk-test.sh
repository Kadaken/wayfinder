#!/usr/bin/env bash
# Private headless GTK test gate. One unique run root under ~/Projects/Scratch,
# removed by THIS parent process on exit, interrupt, terminate, abort or timeout.
#   headless-gtk-test.sh full        the full gtk_headless_suite
#   headless-gtk-test.sh d154        the standalone real-card gate
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

cleanup() {
  trap - EXIT INT TERM
  if [ -n "$child" ]; then
    kill -TERM -- "-$child" 2>/dev/null
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 -- "-$child" 2>/dev/null || break; sleep 0.5; done
    kill -KILL -- "-$child" 2>/dev/null
  fi
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
  --) shift; cmd=("$@");;
  *) echo "usage: $0 full|d154|-- CMD..." >&2; exit 2;;
esac

setsid timeout -k 5 "$t" xvfb-run -a -s "-screen 0 1920x1080x24" "${cmd[@]}" &
child=$!
wait "$child"
st=$?
child=""
exit "$st"
