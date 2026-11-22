#!/bin/bash
# Fixture tests for install-daily.sh. Runs entirely inside a mktemp dir; never touches ~/.local/bin.
set -u
here=$(dirname "$(readlink -f "$0")"); H=$here/install-daily.sh
fx=$(mktemp -d); export HOME=$fx/home; /bin/mkdir -p "$HOME"; trap '/bin/rm -rf -- "${fx:?}"' EXIT
pass=0; fail=0
ok() { if [ "$1" = 0 ]; then pass=$((pass+1)); echo "ok   $2"; else fail=$((fail+1)); echo "FAIL $2"; fi; }
mk() { printf '%s' "$2" > "$1"; /bin/chmod 755 "$1"; }
sha() { sha256sum "$1" | cut -d' ' -f1; }
new_case() {
  c=$fx/$1; /bin/mkdir -p "$c/bin"; T=$c/bin/wayfinder
  mk "$c/new" "new-binary"
  P=$c/approved.sha256
  sha "$c/new" > "$P"
  A=$c/approved.authorization
  printf 'authorized-by Coordinator\ncommit %040d\nsha256 %s\n' 0 "$(sha "$c/new")" > "$A"
}
run_install() { WF_DAILY_APPROVED_SHA256_FILE=$P WF_DAILY_AUTHORIZATION_FILE=$A "$H" "$@"; }

new_case first
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(stat -c %a "$T")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ]; ok $? "first install, no rollback, no leftovers"

new_case repl; mk "$T" old-binary; o=$(sha "$T")
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
B=$T.pre-lab-${o:0:8}
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(sha "$B")" = "$o" ] && [ "$(stat -c %a "$B")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 2 ]; ok $? "replacement keeps verified rollback"

new_case same; mk "$T" old-binary; o=$(sha "$T"); B=$T.pre-lab-${o:0:8}; /bin/cp "$T" "$B"
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(sha "$B")" = "$o" ]; ok $? "byte-identical existing rollback accepted"

new_case conf; mk "$T" old-binary; o=$(sha "$T"); B=$T.pre-lab-${o:0:8}; mk "$B" different; b=$(sha "$B")
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(sha "$B")" = "$b" ]; ok $? "conflicting rollback aborts, target and rollback untouched"

new_case inv; mk "$T" old-binary; o=$(sha "$T")
run_install lab "$c/missing" "$T" >/dev/null 2>&1; r1=$?
run_install lab "$T" "$T" >/dev/null 2>&1; r2=$?
printf 'x WAYFINDER_GUIDE_ONLY_V1 x' > "$c/guide"
run_install lab "$c/guide" "$T" >/dev/null 2>&1; r3=$?
run_install 'bad label' "$c/new" "$T" >/dev/null 2>&1; r4=$?
[ $r1 != 0 ] && [ $r2 != 0 ] && [ $r3 != 0 ] && [ $r4 != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ]; ok $? "missing, self, guide-marked candidates and bad label rejected"

new_case fail; mk "$T" old-binary; o=$(sha "$T")
WF_INSTALL_TEST_FAIL_STAGE=1 run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(stat -c %a "$T")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 2 ]; ok $? "simulated stage failure leaves target unchanged, no stage leftovers"

new_case unapproved; mk "$T" old-binary; o=$(sha "$T"); printf '%064d\n' 0 > "$P"
msg=$(run_install lab "$c/new" "$T" 2>&1); r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ] \
  && [[ $msg == *"candidate $c/new"* ]] && [[ $msg == *"approved pin $P"* ]] \
  && [[ $msg == *"Nothing was written and no rollback was made."* ]]; ok $? "unapproved clean candidate is named and leaves target untouched with no rollback"

new_case auth; mk "$T" old-binary; o=$(sha "$T"); sed -i 's/^sha256 .*/sha256 1111111111111111111111111111111111111111111111111111111111111111/' "$A"
msg=$(run_install lab "$c/new" "$T" 2>&1); r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ] \
  && [[ $msg == *"Coordinator authorization records"* ]] \
  && [[ $msg == *"Nothing was written and no rollback was made."* ]]; ok $? "pin and Coordinator authorization disagreement refuses before any write"

new_case already; /bin/cp "$c/new" "$T"; o=$(sha "$T")
msg=$(run_install lab "$c/new" "$T" 2>&1); r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ] \
  && [[ $msg == *"already installed"* ]] \
  && [[ $msg == *"Nothing was written and no rollback was made."* ]]; ok $? "already-installed approved identity is not rewritten by another round"

# D-390: one install updates every installed program together.
new_case comp; mk "$T" old-binary; mk "$c/bin/wayfinder-native" old-native; mk "$c/bin/wayfinder-portal" old-portal
mk "$c/wayfinder-portal" new-portal; on=$(sha "$c/bin/wayfinder-native"); op=$(sha "$c/bin/wayfinder-portal")
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$T" && cmp -s "$c/new" "$c/bin/wayfinder-native" \
  && cmp -s "$c/wayfinder-portal" "$c/bin/wayfinder-portal" \
  && [ "$(sha "$c/bin/wayfinder-native.pre-lab-${on:0:8}")" = "$on" ] \
  && [ "$(sha "$c/bin/wayfinder-portal.pre-lab-${op:0:8}")" = "$op" ]; ok $? "one install updates wayfinder, wayfinder-native and wayfinder-portal, each with a rollback"

new_case compmiss; mk "$T" old-binary; mk "$c/bin/wayfinder-portal" old-portal; o=$(sha "$T"); p=$(sha "$c/bin/wayfinder-portal")
msg=$(run_install lab "$c/new" "$T" 2>&1); r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(sha "$c/bin/wayfinder-portal")" = "$p" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 2 ] \
  && [[ $msg == *"wayfinder-portal is installed but no new"* ]]; ok $? "installed portal with no new portal beside the candidate refuses before any write"

new_case compstale; /bin/cp "$c/new" "$T"; mk "$c/bin/wayfinder-native" old-native
run_install lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$c/bin/wayfinder-native"; ok $? "stale companion is replaced even when wayfinder is already current"

echo "passed $pass failed $fail"; [ $fail = 0 ]
