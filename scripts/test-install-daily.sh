#!/bin/bash
# Fixture tests for install-daily.sh. Runs entirely inside a mktemp dir; never touches ~/.local/bin.
set -u
here=$(dirname "$(readlink -f "$0")"); H=$here/install-daily.sh
fx=$(mktemp -d); export HOME=$fx/home; /bin/mkdir -p "$HOME"; trap '/bin/rm -rf -- "${fx:?}"' EXIT
pass=0; fail=0
ok() { if [ "$1" = 0 ]; then pass=$((pass+1)); echo "ok   $2"; else fail=$((fail+1)); echo "FAIL $2"; fi; }
mk() { printf '%s' "$2" > "$1"; /bin/chmod 755 "$1"; }
sha() { sha256sum "$1" | cut -d' ' -f1; }
new_case() { c=$fx/$1; /bin/mkdir -p "$c/bin"; T=$c/bin/wayfinder; mk "$c/new" "new-binary"; }

new_case first
"$H" lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(stat -c %a "$T")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ]; ok $? "first install, no rollback, no leftovers"

new_case repl; mk "$T" old-binary; o=$(sha "$T")
"$H" lab "$c/new" "$T" >/dev/null 2>&1; r=$?
B=$T.pre-lab-${o:0:8}
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(sha "$B")" = "$o" ] && [ "$(stat -c %a "$B")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 2 ]; ok $? "replacement keeps verified rollback"

new_case same; mk "$T" old-binary; o=$(sha "$T"); B=$T.pre-lab-${o:0:8}; /bin/cp "$T" "$B"
"$H" lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r = 0 ] && cmp -s "$c/new" "$T" && [ "$(sha "$B")" = "$o" ]; ok $? "byte-identical existing rollback accepted"

new_case conf; mk "$T" old-binary; o=$(sha "$T"); B=$T.pre-lab-${o:0:8}; mk "$B" different; b=$(sha "$B")
"$H" lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(sha "$B")" = "$b" ]; ok $? "conflicting rollback aborts, target and rollback untouched"

new_case inv; mk "$T" old-binary; o=$(sha "$T")
"$H" lab "$c/missing" "$T" >/dev/null 2>&1; r1=$?
"$H" lab "$T" "$T" >/dev/null 2>&1; r2=$?
printf 'x WAYFINDER_GUIDE_ONLY_V1 x' > "$c/guide"
"$H" lab "$c/guide" "$T" >/dev/null 2>&1; r3=$?
"$H" 'bad label' "$c/new" "$T" >/dev/null 2>&1; r4=$?
[ $r1 != 0 ] && [ $r2 != 0 ] && [ $r3 != 0 ] && [ $r4 != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 1 ]; ok $? "missing, self, guide-marked candidates and bad label rejected"

new_case fail; mk "$T" old-binary; o=$(sha "$T")
WF_INSTALL_TEST_FAIL_STAGE=1 "$H" lab "$c/new" "$T" >/dev/null 2>&1; r=$?
[ $r != 0 ] && [ "$(sha "$T")" = "$o" ] && [ "$(stat -c %a "$T")" = 755 ] && [ "$(/bin/ls -A "$c/bin" | wc -l)" = 2 ]; ok $? "simulated stage failure leaves target unchanged, no stage leftovers"

echo "passed $pass failed $fail"; [ $fail = 0 ]
