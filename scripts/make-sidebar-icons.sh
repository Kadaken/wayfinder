#!/usr/bin/env bash
# Rebuild the small sidebar icons from the 96px originals.
#
# D-258. The sidebar draws these at 26 pixels. Handing the window a 96px
# picture and letting it shrink gives a soft edge, because a graphics card
# shrinks with a simple average. Shrinking here with Lanczos and a light
# sharpen keeps the edges hard. Run this after changing any original, or
# after changing SIDEBAR_ICON_SLOT in src-tauri/src/native.rs.
#
#   scripts/make-sidebar-icons.sh [size]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
size="${1:-26}"
out="$here/static/icons/sidebar"
mkdir -p "$out"
for name in desktop documents downloads folders gdrive home music network \
            pictures recent storage trash video; do
    src="$here/static/icons/$name.png"
    [ -f "$src" ] || { echo "missing $src" >&2; exit 1; }
    magick "$src" -filter Lanczos -resize "${size}x${size}" \
        -unsharp 0x0.6+0.8+0.01 "$out/$name.png"
    echo "$name -> ${size}px"
done
