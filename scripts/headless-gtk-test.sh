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
#   headless-gtk-test.sh d226        the standalone no-scroll-flick-on-refresh gate
#   headless-gtk-test.sh d371old     the same gate with the D-371 fix switched off (must FAIL)
#   headless-gtk-test.sh d374old     the window-wider-than-a-monitor gate with the D-374 fix off (must FAIL)
#   headless-gtk-test.sh d375old     the address-bar-right-end gate with the D-375 pin off (must FAIL)
#   headless-gtk-test.sh d377old     the sidebar-toggle-in-the-corner gate with the old placement (must FAIL)
#   headless-gtk-test.sh d377        the sidebar-toggle-in-the-corner gate (size, place, no overlap)
#   headless-gtk-test.sh d382        the standalone .smsbackup-reads-as-a-conversation gate (d382old = old behaviour, must fail)
#   headless-gtk-test.sh d378        the standalone click-inside-the-rename-field-keeps-editing gate (xdotool click, .webm and .txt)
#   headless-gtk-test.sh d375        the standalone address-bar-shows-current-folder gate (xdotool windowsize)
#   headless-gtk-test.sh d374        the standalone window-keeps-its-wide-size gate (xdotool windowsize)
#   headless-gtk-test.sh d371        the standalone delete-keeps-the-view gate (XTEST input, compact list)
#   headless-gtk-test.sh d370        the standalone edge-scroll-while-dragging gate (XTEST input, list and icon view)
#   headless-gtk-test.sh d364        the standalone real-click-keeps-scroll gate (XTEST input, list and icon view)
#   headless-gtk-test.sh d369        the standalone unplayable-video-converts-and-plays gate
#   headless-gtk-test.sh d233        the standalone no-flick-on-permanent-delete gate
#   headless-gtk-test.sh d234        the standalone trash-safety + Trash-row-menu gate
#   headless-gtk-test.sh d235        the standalone Save-picker name-box gate
#   headless-gtk-test.sh -- CMD...   any command (used by the self-check)
# Env: WF_HEADLESS_TIMEOUT seconds (default 1800; the full suite took 971 s on 21 Nov 2026).
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
t="${WF_HEADLESS_TIMEOUT:-1800}"
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
  d226) export WAYFINDER_D226_ONLY=1
        cmd=("${cargo_t[@]}" scroll_flick_d226_alone -- --nocapture);;
  d370) export WAYFINDER_D370_ONLY=1
        cmd=("${cargo_t[@]}" edge_scroll_while_dragging_d370_alone -- --nocapture);;
  d364) export WAYFINDER_D364_ONLY=1
        cmd=("${cargo_t[@]}" pointer_click_keeps_scroll_d364_alone -- --nocapture);;
  d371) export WAYFINDER_D371_ONLY=1
        cmd=("${cargo_t[@]}" delete_keeps_view_d371_alone -- --nocapture);;
  d371old) export WAYFINDER_D371_ONLY=1 WF_TEST_D371_OLD=1
        cmd=("${cargo_t[@]}" delete_keeps_view_d371_alone -- --nocapture);;
  d374) export WAYFINDER_D374_ONLY=1
        cmd=("${cargo_t[@]}" wide_window_keeps_width_d374_alone -- --nocapture);;
  d374old) export WAYFINDER_D374_ONLY=1 WF_TEST_D374_OLD=1
        cmd=("${cargo_t[@]}" wide_window_keeps_width_d374_alone -- --nocapture);;
  d377) export WAYFINDER_D377_ONLY=1
        cmd=("${cargo_t[@]}" sidebar_corner_toggle_d377_alone -- --nocapture);;
  d377old) export WAYFINDER_D377_ONLY=1 WF_TEST_D377_OLD=1
        cmd=("${cargo_t[@]}" sidebar_corner_toggle_d377_alone -- --nocapture);;
  d384old) export WAYFINDER_D377_ONLY=1 WF_TEST_D384_OLD=1
        cmd=("${cargo_t[@]}" sidebar_corner_toggle_d377_alone -- --nocapture);;
  d386) cmd=("${cargo_t[@]}" activating_the_ps2_row_opens_its_folder_d386 -- --nocapture);;
  d385) cmd=("${cargo_t[@]}" png_preview_has_a_visible_size_d316 -- --nocapture);;
  d385old) export WF_TEST_D385_OLD=1
        cmd=("${cargo_t[@]}" png_preview_has_a_visible_size_d316 -- --nocapture);;
  d375) export WAYFINDER_D375_ONLY=1
        cmd=("${cargo_t[@]}" address_scrolled_right_d375_alone -- --nocapture);;
  d382) export WAYFINDER_D382_ONLY=1
        cmd=("${cargo_t[@]}" sms_backup_preview_reads_as_conversation_d382_alone -- --nocapture);;
  d382old) export WAYFINDER_D382_ONLY=1 WF_TEST_D382_OLD=1
        cmd=("${cargo_t[@]}" sms_backup_preview_reads_as_conversation_d382_alone -- --nocapture);;
  d382real) export WAYFINDER_D382_REAL=1
        cmd=("${cargo_t[@]}" sms_backup_ -- --nocapture);;
  d378) export WAYFINDER_D378_ONLY=1
        cmd=("${cargo_t[@]}" rename_click_inside_keeps_editing_d378_alone -- --nocapture);;
  d378old) export WAYFINDER_D378_ONLY=1 WF_TEST_D378_OLD=1
        cmd=("${cargo_t[@]}" rename_click_inside_keeps_editing_d378_alone -- --nocapture);;
  d379) export WAYFINDER_D379_ONLY=1
        cmd=("${cargo_t[@]}" lying_extension_preview_d379_alone -- --nocapture);;
  d379old) export WAYFINDER_D379_ONLY=1 WF_TEST_D379_OLD=1
        cmd=("${cargo_t[@]}" lying_extension_preview_d379_alone -- --nocapture);;
  d380) export WAYFINDER_D380_ONLY=1
        cmd=("${cargo_t[@]}" doc_bar_narrow_pane_d380_alone -- --nocapture);;
  d380old) export WAYFINDER_D380_ONLY=1 WF_TEST_D380_OLD=1
        cmd=("${cargo_t[@]}" doc_bar_narrow_pane_d380_alone -- --nocapture);;
  d381) export WAYFINDER_D381_ONLY=1
        cmd=("${cargo_t[@]}" arrow_key_image_preview_d381_alone -- --nocapture);;
  d381old) export WAYFINDER_D381_ONLY=1 WF_TEST_D381_OLD=1
        cmd=("${cargo_t[@]}" arrow_key_image_preview_d381_alone -- --nocapture);;
  d389) export WAYFINDER_D389_ONLY=1
        cmd=("${cargo_t[@]}" held_arrow_key_image_preview_d389_alone -- --nocapture);;
  d383) export WAYFINDER_D383_ONLY=1
        cmd=("${cargo_t[@]}" fix_extension_d383_alone -- --nocapture);;
  d383old) export WAYFINDER_D383_ONLY=1 WF_TEST_D383_OLD=1
        cmd=("${cargo_t[@]}" fix_extension_d383_alone -- --nocapture);;
  d375old) export WAYFINDER_D375_ONLY=1 WF_TEST_D375_OLD=1
        cmd=("${cargo_t[@]}" address_scrolled_right_d375_alone -- --nocapture);;
  d33) export WAYFINDER_D33_ONLY=1
        cmd=("${cargo_t[@]}" folder_activation_d33_alone -- --nocapture);;
  d20) export WAYFINDER_D20_ONLY=1
        cmd=("${cargo_t[@]}" emptied_folder_focus_d20_alone -- --nocapture);;
  d368) export WAYFINDER_D368_ONLY=1
        cmd=("${cargo_t[@]}" grid_thumbnails_past_60_d368_alone -- --nocapture);;
  d369) export WAYFINDER_D369_ONLY=1
        cmd=("${cargo_t[@]}" unplayable_video_alone_d369 -- --nocapture);;
  d233) export WAYFINDER_D233_ONLY=1
        cmd=("${cargo_t[@]}" scroll_flick_d233_alone -- --nocapture);;
  d234) export WAYFINDER_D234_ONLY=1
        cmd=("${cargo_t[@]}" trash_button_in_trash_asks_first_d234_alone -- --nocapture);;
  d235) export WAYFINDER_D235_ONLY=1
        cmd=("${cargo_t[@]}" save_picker_row_click_copies_the_name_without_recursing_d235_alone -- --nocapture);;
  d239) export WAYFINDER_D239_ONLY=1
        cmd=("${cargo_t[@]}" browsing_and_tabs_watched_d239_alone -- --nocapture);;
  d237) export WAYFINDER_D237_ONLY=1
        cmd=("${cargo_t[@]}" save_picker_opens_a_folder_instead_of_closing_d237_alone -- --nocapture);;
  d260) export WAYFINDER_D260_ONLY=1
        cmd=("${cargo_t[@]}" details_thumbnail_regression_alone_d260 -- --nocapture);;
  d261) export WAYFINDER_D261_ONLY=1
        cmd=("${cargo_t[@]}" image_formats_alone_d261 -- --nocapture);;
  d262) export WAYFINDER_D262_ONLY=1
        cmd=("${cargo_t[@]}" pasted_address_alone_d262 -- --nocapture);;
  d263) export WAYFINDER_D263_ONLY=1
        cmd=("${cargo_t[@]}" odd_picture_thumbnail_alone_d263 -- --nocapture);;
  d264) export WAYFINDER_D264_ONLY=1
        cmd=("${cargo_t[@]}" open_at_file_alone_d264 -- --nocapture);;
  d267) export WAYFINDER_D267_ONLY=1
        cmd=("${cargo_t[@]}" grid_columns_alone_d267 -- --nocapture);;
  d268) export WAYFINDER_D268_ONLY=1
        cmd=("${cargo_t[@]}" folder_preview_alone_d268 -- --nocapture);;
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
  *) echo "usage: $0 full|d154|d53|d155|d185|d186|d188|d187|d226|d364|d370|d371|d371old|d374|d374old|d375|d375old|d378|d379|d379old|d380|d380old|d381|d381old|d389|d383|d383old|d382|d382old|d382real|d377|d377old|d384old|d385|d385old|d386|d368|d369|d234|d235|d237|d239|-- CMD..." >&2; exit 2;;
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
