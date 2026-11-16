#!/usr/bin/env bash
# Rebuild the small sidebar icons from the 96px originals.
#
# D-258. Handing the window a big picture and letting it shrink gives a soft
# edge, because a graphics card shrinks with a simple average. Shrinking here
# with Lanczos and a light sharpen keeps the edges hard. Run this after
# changing any original.
#
# D-275. The size is read from SIDEBAR_ICON_SLOT in src-tauri/src/native.rs
# rather than typed here, so the two cannot drift. They did: 0.9.22 put full
# size art straight into static/icons/sidebar, and every sidebar icon since
# has been shrunk by the graphics card instead -- soft again, and tall enough
# to stretch the rows whenever the sidebar had room.
#
# D-277. The masters are static/icons/sidebar-src, NOT static/icons. The two
# are different drawings: static/icons still holds the first 96px set, and
# 0.9.22 replaced the sidebar art with a new 256px set without touching them.
# Shrinking from static/icons therefore shipped the old drawings in 0.9.25.
#
#   scripts/make-sidebar-icons.sh [size]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
slot="$(sed -n 's/^const SIDEBAR_ICON_SLOT: i32 = \([0-9]*\);/\1/p' \
    "$here/src-tauri/src/native.rs")"
[ -n "$slot" ] || { echo "cannot read SIDEBAR_ICON_SLOT from native.rs" >&2; exit 1; }
size="${1:-$slot}"
out="$here/static/icons/sidebar"
mkdir -p "$out"
for name in desktop documents downloads folders gdrive home music network \
            pictures recent storage trash video; do
    src="$here/static/icons/sidebar-src/$name.png"
    [ -f "$src" ] || { echo "missing $src" >&2; exit 1; }
    magick "$src" -filter Lanczos -resize "${size}x${size}" \
        -unsharp 0x0.6+0.8+0.01 "$out/$name.png"
    echo "$name -> ${size}px"
done
