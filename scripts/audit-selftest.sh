#!/usr/bin/env bash
# Self-test for scripts/audit-mechanical.sh.
#
# WHY THIS EXISTS: a check that has never failed is not a check. Every PASS line
# the mechanical audit prints is only worth reading if the same line is known to
# go FAIL when the thing it guards is actually broken. This script breaks one
# thing at a time in a throwaway clone and asserts the matching check ID flips.
#
# The fake secret below is assembled at runtime from two pieces on purpose. A
# literal AKIA key in this file would make the real audit's K6 check fire on the
# self-test itself — which it did, the first time this script was committed.
#
# It never touches the real working tree. It clones into a scratch directory,
# mutates the clone, runs the audit against it with --repo, and resets.
#
# Usage:  scripts/audit-selftest.sh [--keep]
#   --keep   leave the scratch clone in place for inspection
#
# Exit 0 = every case behaved. Exit 1 = at least one check did not fire, which
# is a defect in the audit, not in the app.
set -uo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEEP=0; [[ "${1:-}" == "--keep" ]] && KEEP=1
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wf-audit-selftest.XXXXXX")"
CLONE="$WORK/repo"
cleanup(){ (( KEEP )) && { echo "scratch kept: $WORK"; return; }; rm -rf "$WORK"; }
trap cleanup EXIT

echo "=== audit self-test"
echo "source: $SRC"
echo "clone:  $CLONE"
git clone --quiet --local --no-hardlinks "$SRC" "$CLONE" || { echo "clone failed"; exit 2; }
cd "$CLONE" || exit 2
git config user.email selftest@invalid
git config user.name selftest
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
echo "version: $VER"
echo

OK=0; BAD=0; NOTRUN=0
declare -a FAILED_CASES=()

reset_clone(){
  cd "$CLONE" || exit 2
  git checkout --quiet -- . 2>/dev/null
  git clean -qfd 2>/dev/null
  git reset --quiet --hard "$BASE" 2>/dev/null
  rm -rf src-tauri/target/release 2>/dev/null
  # The baseline must pass S8.3, so the tag sits on the commit under test.
  git tag -f "v$VER" "$BASE" >/dev/null 2>&1
}

run_audit(){ bash "$CLONE/scripts/audit-mechanical.sh" --repo "$CLONE" 2>&1; }

# A case: label, the check ID that must go FAIL, and the breakage.
case_expect_fail(){
  local id="$1" label="$2"; shift 2
  reset_clone
  if ! "$@"; then
    printf 'NOTRUN %-9s %s (breakage could not be applied)\n' "$id" "$label"
    NOTRUN=$((NOTRUN+1)); return
  fi
  local out; out="$(run_audit)"
  if grep -qE "^FAIL   $id " <<<"$out"; then
    printf 'FIRED  %-9s %s\n' "$id" "$label"
    OK=$((OK+1))
  else
    printf 'MISS   %-9s %s — the check did NOT fail\n' "$id" "$label"
    printf '         its line was: %s\n' "$(grep -E "^[A-Z]+ +$id " <<<"$out" | head -1 | sed 's/  */ /g')"
    BAD=$((BAD+1)); FAILED_CASES+=("$id $label")
  fi
}

BASE="$(git rev-parse HEAD)"

# ---------------------------------------------------------------- the baseline
reset_clone
echo "-- baseline: an untouched clone with the tag on HEAD must have zero FAIL"
base_out="$(run_audit)"
base_fails="$(grep -c "^FAIL " <<<"$base_out")"
if (( base_fails == 0 )); then
  printf 'FIRED  %-9s baseline is clean, so every later FAIL is caused by the breakage\n' "-"
  OK=$((OK+1))
else
  printf 'MISS   %-9s baseline already has %s FAIL lines — the cases below are not isolated\n' "-" "$base_fails"
  grep "^FAIL " <<<"$base_out" | sed 's/^/         /'
  BAD=$((BAD+1)); FAILED_CASES+=("- baseline not clean")
fi
echo

echo "-- negative cases: break one thing, assert the matching ID flips"

case_expect_fail S8.0 "no version in Cargo.toml" \
  bash -c 'sed -i "0,/^version = /s/^version = .*/version = \"\"/" src-tauri/Cargo.toml'

case_expect_fail S8.1 "SECURITY.md no longer carries the version" \
  bash -c 'sed -i "s/[0-9]\+\.[0-9]\+\.[0-9]\+/0.0.0-wrong/g" SECURITY.md'

case_expect_fail S8.3 "tag points at a different commit than HEAD" \
  bash -c 'git tag -f "v'"$VER"'" HEAD~1 >/dev/null 2>&1'

case_expect_fail S8.5 "an uncommitted change in the tree" \
  bash -c 'echo "# selftest" >> README.md'

case_expect_fail S15.1 "a dirty tree at the build commit" \
  bash -c 'echo "# selftest" >> README.md'

case_expect_fail S15.2 "Cargo.lock untracked" \
  bash -c 'git rm --quiet --cached src-tauri/Cargo.lock'

case_expect_fail S18.1 "a desktop file with a real error in it" \
  bash -c 'sed -i "/^Type=Application/d" packaging/com.kadaken.wayfinder.desktop'

case_expect_fail S18.2 "malformed metainfo XML" \
  bash -c 'printf "<not-closed>\n" >> src-tauri/com.kadaken.wayfinder.metainfo.xml'

case_expect_fail S18.3 "newest <release> is not this version" \
  bash -c 'sed -i "0,/<release version=/s/<release version=\"[^\"]*\"/<release version=\"9.9.9\"/" src-tauri/com.kadaken.wayfinder.metainfo.xml'

case_expect_fail S5.1 "declared licence disagrees with LICENSE" \
  bash -c 'sed -i "s/^license = .*/license = \"MIT\"/" src-tauri/Cargo.toml'

case_expect_fail K6.1 "a certain-secret pattern in a tracked file" \
  bash -c 'printf "%s%s\n" "AKIA" "QQQQQQQQQQQQQQQQ" > selftest-leak.txt && git add selftest-leak.txt'

case_expect_fail K6.3 "a secret committed and then deleted in a later commit" \
  bash -c 'printf "%s%s\n" "AKIA" "QQQQQQQQQQQQQQQQ" > selftest-leak.txt && git add selftest-leak.txt \
           && git commit --quiet -m "selftest add leak" && git rm --quiet selftest-leak.txt \
           && git commit --quiet -m "selftest remove leak" && git tag -f "v'"$VER"'" HEAD >/dev/null 2>&1'

case_expect_fail S2.1 "the builder home path in a tracked file" \
  bash -c 'printf "path = \"%s/secret\"\n" "$HOME" > selftest-path.txt && git add selftest-path.txt'

case_expect_fail S2.2 "the builder home path inside a shipped binary" \
  bash -c 'mkdir -p src-tauri/target/release \
           && printf "junk %s/build/path junk\n" "$HOME" > src-tauri/target/release/wayfinder-native \
           && chmod +x src-tauri/target/release/wayfinder-native'

case_expect_fail W7.1 "a tracked file under dist/" \
  bash -c 'mkdir -p dist && printf x > dist/selftest.txt && git add -f dist/selftest.txt'

case_expect_fail W7.2 "dist/ removed from .gitignore" \
  bash -c 'sed -i "/^dist\/$/d" .gitignore'

case_expect_fail W5.1 "the installer is not executable" \
  bash -c 'chmod -x packaging/install.sh'

case_expect_fail W5.2 "the installer lost its --dry-run flag" \
  bash -c 'sed -i "s/--dry-run/--selftest-removed/g" packaging/install.sh'

case_expect_fail W5.3 "the flatpak manifest carries a stale tag" \
  bash -c 'sed -i "s/tag: v[0-9.]*/tag: v0.0.1/" flatpak/com.kadaken.wayfinder.yml'

# ---------------------------------------------------------------- the judged half
echo
echo "-- the judged half: an unanswered check must block, an answered one must clear"
reset_clone
out="$(run_audit)"; rc_open=0; grep -qE "^OPEN   S1 " <<<"$out" && rc_open=1
run_audit >/dev/null 2>&1; exit_with_open=$?
if (( rc_open == 1 && exit_with_open != 0 )); then
  printf 'FIRED  %-9s an unanswered judged check prints OPEN and exits non-zero\n' "ledger"
  OK=$((OK+1))
else
  printf 'MISS   %-9s unanswered judged check did not block (OPEN=%s exit=%s)\n' "ledger" "$rc_open" "$exit_with_open"
  BAD=$((BAD+1)); FAILED_CASES+=("ledger unanswered did not block")
fi

reset_clone
mkdir -p audit
printf 'S1 @ %s — selftest answer (1970-01-01)\n' "$VER" >> audit/ANSWERS.md
out="$(run_audit)"
if grep -qE "^ANSWERED S1 " <<<"$out" && ! grep -qE "^OPEN   S1 " <<<"$out"; then
  printf 'FIRED  %-9s a recorded answer clears the check\n' "ledger"
  OK=$((OK+1))
else
  printf 'MISS   %-9s a recorded answer did NOT clear the check\n' "ledger"
  BAD=$((BAD+1)); FAILED_CASES+=("ledger answer did not clear")
fi

reset_clone
echo
echo "-- not covered by this self-test, and why"
echo "   S8.4   tag commit newer than HEAD — needs a forged committer date"
echo "   K8.1   the VULNERABILITY branch — needs a dependency with a real advisory"
echo "   S18.1  the menu-placement hint branch — it is a HINT by design, not a FAIL"
echo "   every JUDGE id — not mechanical, so there is nothing to fire"
echo
echo "=== $OK fired, $BAD did not, $NOTRUN could not be set up"
if (( BAD )); then
  echo "defects in the audit:"
  for c in "${FAILED_CASES[@]}"; do echo "   $c"; done
  exit 1
fi
(( NOTRUN )) && exit 1
exit 0
