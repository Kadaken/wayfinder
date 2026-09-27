#!/bin/bash
# usage: install-daily.sh <change-label> <candidate-binary> [target-override]
# The only sanctioned way to replace ~/.local/bin/wayfinder.
# Hard rule: no daily replacement without a verified rollback copy first.
set -u
label=${1:-}; cand=${2:-}; target=${3:-$HOME/.local/bin/wayfinder}
die() { echo "FAIL: $*" >&2; exit 1; }
[ $# -ge 2 ] && [ $# -le 3 ] || { echo "usage: $0 <change-label> <candidate-binary> [target-override]" >&2; exit 2; }
[[ $label =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "bad change label: $label"
[ -f "$cand" ] && [ -r "$cand" ] || die "cannot read candidate: $cand"
[ ! -e "$target" ] || [ ! "$cand" -ef "$target" ] || die "candidate is the installed daily itself"
here=$(dirname "$(readlink -f "$0")")
"$here/check-default-release.sh" "$cand" >/dev/null || die "candidate is not a clean default release"
dir=$(dirname "$target"); [ -d "$dir" ] && [ -w "$dir" ] || die "target directory not writable: $dir"
sha() { sha256sum -- "$1" | cut -d' ' -f1; }
tmps=()
trap '/bin/rm -f -- "${tmps[@]}"' EXIT
if [ -e "$target" ]; then
  [ -f "$target" ] || die "target is not a regular file: $target"
  old=$(sha "$target"); mode=$(stat -c %a "$target")
  bak="$target.pre-$label-${old:0:8}"
  if [ -e "$bak" ]; then
    [ -f "$bak" ] && cmp -s "$target" "$bak" && [ "$(sha "$bak")" = "$old" ] && [ "$(stat -c %a "$bak")" = "$mode" ] \
      || die "rollback file exists and differs, never overwritten: $bak"
    echo "rollback already present and verified: $bak"
  else
    t=$(mktemp "$dir/.rollback.XXXXXX") || die "cannot stage rollback"; tmps+=("$t")
    /bin/cp -- "$target" "$t" && /bin/chmod "$mode" "$t" || die "rollback copy failed"
    cmp -s "$target" "$t" && [ "$(sha "$t")" = "$old" ] && [ "$(stat -c %a "$t")" = "$mode" ] || die "rollback verify failed"
    ln -T -- "$t" "$bak" 2>/dev/null || die "rollback name taken, never overwritten: $bak"
    cmp -s "$target" "$bak" && [ "$(sha "$bak")" = "$old" ] && [ "$(stat -c %a "$bak")" = "$mode" ] || die "rollback verify failed after link: $bak"
    echo "rollback created and verified: $bak"
  fi
else
  echo "no existing target: first install, no rollback needed"
fi
new=$(mktemp "$dir/.stage.XXXXXX") || die "cannot stage candidate"; tmps+=("$new")
/bin/cp -- "$cand" "$new" && /bin/chmod 755 "$new" || die "stage copy failed"
[ -z "${WF_INSTALL_TEST_FAIL_STAGE:-}" ] || die "simulated stage failure"
cmp -s "$cand" "$new" && [ "$(sha "$new")" = "$(sha "$cand")" ] && [ "$(stat -c %a "$new")" = 755 ] || die "stage verify failed"
/bin/mv -T -- "$new" "$target" || die "atomic rename failed"
[ "$(sha "$target")" = "$(sha "$cand")" ] || die "post-install hash mismatch (rollback is safe)"
echo "installed $target sha256 $(sha "$target")"
