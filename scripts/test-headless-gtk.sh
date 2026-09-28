#!/usr/bin/env bash
# Self-check for headless-gtk-test.sh: a synthetic abort and a synthetic
# timeout must both leave their exact run root gone, and neither may touch
# an unrelated sibling tree.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
w="$here/headless-gtk-test.sh"
scratch="$HOME/Projects/Scratch"
sib="$(mktemp -d "$scratch/wf-headless-sibling-XXXXXX")"
fail=0
check() { # name expected_status cmd...
  local name="$1" want="$2"; shift 2
  local out st root
  out="$(WF_HEADLESS_TIMEOUT="${TO:-60}" "$w" -- "$@" 2>&1)"; st=$?
  root="$(printf '%s\n' "$out" | sed -n 's/^RUN_ROOT=//p' | head -1)"
  if [ -z "$root" ] || [ -e "$root" ]; then echo "FAIL $name: root '$root' still exists"; fail=1
  elif [ "$want" = nonzero ] && [ "$st" -eq 0 ]; then echo "FAIL $name: status 0"; fail=1
  else echo "ok $name status=$st root gone ($root)"; fi
}
check normal 0 sh -c 'echo x > "$WF_TEST_SCRATCH_ROOT/f"'
check assertion-failure nonzero sh -c 'echo x > "$WF_TEST_SCRATCH_ROOT/f"; exit 1'
check abort nonzero sh -c 'mkdir "$WF_TEST_SCRATCH_ROOT/d"; echo x > "$WF_TEST_SCRATCH_ROOT/d/f"; kill -ABRT $$'
TO=2 check timeout nonzero sh -c 'echo x > "$WF_TEST_SCRATCH_ROOT/f"; sleep 30'
if [ -d "$sib" ]; then echo "ok sibling untouched"; rmdir "$sib"; else echo "FAIL sibling removed"; fail=1; fi
exit "$fail"
