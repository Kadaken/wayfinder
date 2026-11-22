#!/bin/bash
# usage: install-daily.sh <change-label> <candidate-binary> [target-override]
# The only sanctioned way to replace ~/.local/bin/wayfinder.
# Also replaces wayfinder-native and wayfinder-portal when installed beside it (D-390).
# Hard rules: the real daily accepts only the repository-pinned identity, and
# no daily replacement happens without a verified rollback copy first.
set -u
account_home=$(getent passwd "$(id -u)" | cut -d: -f6)
[ -n "$account_home" ] || { echo "FAIL: cannot resolve account home" >&2; exit 1; }
production_target=$(readlink -m "$account_home/.local/bin/wayfinder")
label=${1:-}; cand=${2:-}; target=${3:-$production_target}
die() { echo "FAIL: $*" >&2; exit 1; }
refuse_before_write() {
  echo "REFUSED: candidate $cand; approved pin $approved_file; $*. Nothing was written and no rollback was made." >&2
  exit 1
}
[ $# -ge 2 ] && [ $# -le 3 ] || { echo "usage: $0 <change-label> <candidate-binary> [target-override]" >&2; exit 2; }
[[ $label =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "bad change label: $label"
[ -f "$cand" ] && [ -r "$cand" ] || die "cannot read candidate: $cand"
[ ! -e "$target" ] || [ ! "$cand" -ef "$target" ] || die "candidate is the installed daily itself"
here=$(dirname "$(readlink -f "$0")")
"$here/check-default-release.sh" "$cand" >/dev/null || die "candidate is not a clean default release"
sha() { sha256sum -- "$1" | cut -d' ' -f1; }
target_path=$(readlink -m "$target")
if [ "$target_path" = "$production_target" ]; then
  approved_file=$here/daily-approved.sha256
  authorization_file=$here/daily-approved.authorization
  [ -z "${WF_DAILY_APPROVED_SHA256_FILE:-}${WF_DAILY_AUTHORIZATION_FILE:-}" ] \
    || refuse_before_write "the production approval cannot be overridden"
else
  approved_file=${WF_DAILY_APPROVED_SHA256_FILE:-$here/daily-approved.sha256}
  authorization_file=${WF_DAILY_AUTHORIZATION_FILE:-$here/daily-approved.authorization}
fi
[ -f "$approved_file" ] && [ -r "$approved_file" ] \
  || refuse_before_write "the pin cannot be read"
approved=$(tr -d '\n' < "$approved_file")
[[ $approved =~ ^[0-9a-f]{64}$ ]] \
  || refuse_before_write "the pin is not exactly one lowercase SHA-256"
[ -f "$authorization_file" ] && [ -r "$authorization_file" ] \
  || refuse_before_write "Coordinator authorization $authorization_file cannot be read"
mapfile -t authorization < "$authorization_file"
[ ${#authorization[@]} -eq 3 ] \
  && [ "${authorization[0]}" = "authorized-by Coordinator" ] \
  && [[ ${authorization[1]} =~ ^commit\ [0-9a-f]{40}$ ]] \
  && [[ ${authorization[2]} =~ ^sha256\ [0-9a-f]{64}$ ]] \
  || refuse_before_write "Coordinator authorization $authorization_file is malformed"
authorization_sha=${authorization[2]#sha256 }
[ "$authorization_sha" = "$approved" ] \
  || refuse_before_write "the pin records $approved but Coordinator authorization records $authorization_sha"
candidate_sha=$(sha "$cand")
[ "$candidate_sha" = "$approved" ] \
  || refuse_before_write "candidate SHA-256 $candidate_sha does not match approved SHA-256 $approved"
dir=$(dirname "$target"); [ -d "$dir" ] && [ -w "$dir" ] || die "target directory not writable: $dir"
# D-390: one install updates every installed program together. The portal
# launches the sibling wayfinder-native for every file dialog, so a stale
# wayfinder-native or wayfinder-portal next to a new wayfinder ran old code.
# Companions already present in the target directory are replaced in the same
# run: wayfinder-native gets the pinned candidate bytes, wayfinder-portal the
# one beside the candidate (required whenever a portal is installed).
cand_dir=$(dirname "$cand")
names=("$target"); srcs=("$cand")
for comp in wayfinder-native wayfinder-portal; do
  [ -e "$dir/$comp" ] || continue
  [ "$dir/$comp" != "$target" ] || continue
  if [ "$comp" = wayfinder-native ]; then csrc=$cand; else csrc=$cand_dir/$comp; fi
  [ -f "$csrc" ] && [ -r "$csrc" ] \
    || refuse_before_write "$comp is installed but no new $comp was found at $csrc"
  "$here/check-default-release.sh" "$csrc" >/dev/null \
    || refuse_before_write "$csrc is not a clean default release"
  [ -f "$dir/$comp" ] || die "target is not a regular file: $dir/$comp"
  names+=("$dir/$comp"); srcs+=("$csrc")
done
todo=(); todo_src=()
for i in "${!names[@]}"; do
  if [ -e "${names[$i]}" ] && [ "$(sha "${names[$i]}")" = "$(sha "${srcs[$i]}")" ]; then continue; fi
  todo+=("${names[$i]}"); todo_src+=("${srcs[$i]}")
done
[ ${#todo[@]} -gt 0 ] \
  || refuse_before_write "that approved identity is already installed"
tmps=()
trap '/bin/rm -f -- "${tmps[@]}"' EXIT
staged=()
for i in "${!todo[@]}"; do
  t_target=${todo[$i]}; t_src=${todo_src[$i]}; t_sha=$(sha "$t_src")
  if [ -e "$t_target" ]; then
    [ -f "$t_target" ] || die "target is not a regular file: $t_target"
    old=$(sha "$t_target"); mode=$(stat -c %a "$t_target")
    bak="$t_target.pre-$label-${old:0:8}"
    if [ -e "$bak" ]; then
      [ -f "$bak" ] && cmp -s "$t_target" "$bak" && [ "$(sha "$bak")" = "$old" ] && [ "$(stat -c %a "$bak")" = "$mode" ] \
        || die "rollback file exists and differs, never overwritten: $bak"
      echo "rollback already present and verified: $bak"
    else
      t=$(mktemp "$dir/.rollback.XXXXXX") || die "cannot stage rollback"; tmps+=("$t")
      /bin/cp -- "$t_target" "$t" && /bin/chmod "$mode" "$t" || die "rollback copy failed"
      cmp -s "$t_target" "$t" && [ "$(sha "$t")" = "$old" ] && [ "$(stat -c %a "$t")" = "$mode" ] || die "rollback verify failed"
      ln -T -- "$t" "$bak" 2>/dev/null || die "rollback name taken, never overwritten: $bak"
      cmp -s "$t_target" "$bak" && [ "$(sha "$bak")" = "$old" ] && [ "$(stat -c %a "$bak")" = "$mode" ] || die "rollback verify failed after link: $bak"
      echo "rollback created and verified: $bak"
    fi
  else
    echo "no existing target: first install, no rollback needed"
  fi
  new=$(mktemp "$dir/.stage.XXXXXX") || die "cannot stage candidate"; tmps+=("$new")
  /bin/cp -- "$t_src" "$new" && /bin/chmod 755 "$new" || die "stage copy failed"
  [ -z "${WF_INSTALL_TEST_FAIL_STAGE:-}" ] || die "simulated stage failure"
  cmp -s "$t_src" "$new" && [ "$(sha "$new")" = "$t_sha" ] && [ "$(stat -c %a "$new")" = 755 ] || die "stage verify failed"
  staged+=("$new")
done
# Every program is staged and verified; only now does any rename happen.
for i in "${!todo[@]}"; do
  /bin/mv -T -- "${staged[$i]}" "${todo[$i]}" || die "atomic rename failed: ${todo[$i]}"
  [ "$(sha "${todo[$i]}")" = "$(sha "${todo_src[$i]}")" ] || die "post-install hash mismatch (rollback is safe): ${todo[$i]}"
  echo "installed ${todo[$i]} sha256 $(sha "${todo[$i]}")"
done
