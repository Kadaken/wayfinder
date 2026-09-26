#!/bin/bash
# usage: check-default-release.sh <release-binary>
# Fails if the binary is missing, unreadable, or contains any guide-only marker.
set -u
bin=${1:-}
[ -n "$bin" ] || { echo "usage: $0 <release-binary>" >&2; exit 2; }
[ -f "$bin" ] && [ -r "$bin" ] || { echo "FAIL: cannot read binary: $bin" >&2; exit 2; }
markers=('/.local/share/crew/guide/' 'WF_GUIDE_FILE' 'WAYFINDER_GUIDE_ONLY_V1' 'WAYFINDER_GUIDE_CIRCLE_V1' 'WAYFINDER_GUIDE_ARROW_V1')
bad=0
for m in "${markers[@]}"; do
  if grep -aqF -- "$m" "$bin"; then echo "FAIL: forbidden guide marker present: $m" >&2; bad=1; fi
done
[ "$bad" = 0 ] || exit 1
echo "OK: no guide markers in $bin"
