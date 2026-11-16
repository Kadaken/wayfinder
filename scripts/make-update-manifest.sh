#!/usr/bin/env bash
# Writes wayfinder-update.json, the file the in-app update check reads.
#
# It is generated off the release tarball rather than typed, because a
# hand-typed version label is the exact fault the site already carries: the
# comment at the top of kadaken.com's functions/versions.js records labels
# sitting four releases behind what was actually live, and a stale checksum
# here would be worse than a stale label -- it would make every update refuse
# to install.
#
# Everything in the manifest comes from one of two places: Cargo.toml, or the
# tarball itself. Nothing is passed in by hand except the release note.
#
#   ./scripts/make-update-manifest.sh                       uses dist/
#   ./scripts/make-update-manifest.sh --tarball PATH        a specific archive
#   ./scripts/make-update-manifest.sh --notes "one line"    the dialog's text
#   ./scripts/make-update-manifest.sh --out DIR             where to write it
#
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 LegendofZito (Kadaken)

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tarball=""
notes=""
out="$root/dist"

while [ $# -gt 0 ]; do
  case "$1" in
    --tarball) tarball="${2:?--tarball needs a path}"; shift 2 ;;
    --notes)   notes="${2-}"; shift 2 ;;
    --out)     out="${2:?--out needs a directory}"; shift 2 ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "no version in src-tauri/Cargo.toml" >&2; exit 1; }

if [ -z "$tarball" ]; then
  tarball="$root/dist/wayfinder-$version-$(uname -m)-linux.tar.gz"
fi
[ -f "$tarball" ] || {
  echo "no tarball at $tarball" >&2
  echo "run scripts/make-release-tarball.sh first" >&2
  exit 1
}

# The archive must be the version the manifest claims. Publishing a manifest
# that says 0.9.24 beside a 0.9.23 tarball would have every client download the
# old build, see the old version, and offer the update again forever.
# `| head -1` would make tar die of SIGPIPE, and `pipefail` turns that into a
# failed script, so the whole listing is read and the first line taken from it.
inside="$(tar tzf "$tarball")"
inside="${inside%%$'\n'*}"
inside="${inside%%/*}"
case "$inside" in
  "wayfinder-$version-"*) ;;
  *) echo "the tarball unpacks to '$inside', which is not version $version" >&2; exit 1 ;;
esac

size="$(stat -c %s "$tarball")"
sha="$(sha256sum "$tarball" | cut -d' ' -f1)"

mkdir -p "$out"
manifest="$out/wayfinder-update.json"
# The url is the counted /get/ route, not the raw file, so an update taken
# in-app shows up in the download figures the same as one taken from the page.
command cat > "$manifest" <<JSON
{
  "version": "$version",
  "date": "$(date -u +%Y-%m-%d)",
  "url": "https://kadaken.com/get/wayfinder",
  "sha256": "$sha",
  "size": $size,
  "notes": $(printf '%s' "$notes" | python3 -I -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
}
JSON

printf 'wrote %s\n' "$manifest"
printf '  version %s, %s bytes\n' "$version" "$size"
printf '  sha256  %s\n' "$sha"
