#!/usr/bin/env bash
# Build the download that goes on the website and on the GitHub release.
#
# The tarball mirrors the repository's own paths, because packaging/install.sh
# finds everything relative to its own parent: the binaries under
# src-tauri/target/release, the icons under src-tauri/icons, the New Window
# helper under scripts. Unpack it anywhere and ./packaging/install.sh works
# exactly as it does in a clone, with no toolchain installed.
#
#   scripts/make-release-tarball.sh            -> dist/wayfinder-<ver>-x86_64-linux.tar.gz
#   scripts/make-release-tarball.sh --out DIR  -> somewhere else
#
# Writes the archive and SHA256SUMS.txt beside it, the two names the earlier
# releases used.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$here/.." && pwd)"
out="$root/dist"
[ "${1:-}" = "--out" ] && { out="$2"; shift 2; }

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "cannot read the version out of src-tauri/Cargo.toml" >&2; exit 1; }
arch="$(uname -m)"

# D-435: a release is blocked unless the shipped programs are clean under
# warnings-denied clippy. Test code is not shipped and is not gated here.
clippy_target="${CARGO_TARGET_DIR:-$root/src-tauri/target}"
systemd-run --user --scope -q -p MemoryMax=4G -p MemorySwapMax=0 \
  env CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$clippy_target" \
  cargo clippy --bins --locked --manifest-path "$root/src-tauri/Cargo.toml" -- -D warnings

# Same shape as the 0.9.1-0.9.3 releases, so the URL pattern never changes.
name="wayfinder-$version-$arch-linux"

bin="$root/src-tauri/target/release"
for b in wayfinder-native wayfinder-portal; do
  [ -x "$bin/$b" ] || { echo "no release binary at $bin/$b -- run cargo build --release --bins" >&2; exit 1; }
done

# A binary that names the machine it was built on must never leave here. The
# remap flags are applied at build time (scripts/release-rustflags.sh); this is
# the check that they actually were.
leaked=0
for b in wayfinder-native wayfinder-portal; do
  for probe in "${CARGO_HOME:-$HOME/.cargo}" "${RUSTUP_HOME:-$HOME/.rustup}" "$HOME" "$root"; do
    n="$(strings -a "$bin/$b" | grep -c -F -- "$probe" || true)"
    [ "$n" = 0 ] && continue
    echo "$b carries the build path '$probe' $n times" >&2
    leaked=1
  done
done
if [ "$leaked" = 1 ]; then
  cat >&2 <<'MSG'
refusing to package: rebuild with the remap flags first --
  cd src-tauri
  CARGO_ENCODED_RUSTFLAGS="$(../scripts/release-rustflags.sh)" cargo build --release --bins
MSG
  exit 1
fi

# A build from a dirty tree would ship something no commit describes.
if [ -z "${WAYFINDER_RELEASE_ALLOW_DIRTY:-}" ] && ! git -C "$root" diff --quiet HEAD -- src-tauri/src packaging scripts static; then
  echo "the tree has uncommitted changes under src-tauri/src, packaging, scripts or static;" >&2
  echo "commit them first, or set WAYFINDER_RELEASE_ALLOW_DIRTY=1 to override" >&2
  exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf -- "${stage:?}"' EXIT
top="$stage/$name"
mkdir -p "$top/packaging" "$top/scripts" "$top/src-tauri/target/release" "$top/src-tauri/icons"

cp -p "$root"/packaging/* "$top/packaging/"
cp -p "$root/scripts/wayfinder-new-window" "$top/scripts/"
cp -p "$bin/wayfinder-native" "$bin/wayfinder-portal" "$top/src-tauri/target/release/"
for icon in 32x32.png 64x64.png 128x128.png '128x128@2x.png' icon.png icon.svg; do
  cp -p "$root/src-tauri/icons/$icon" "$top/src-tauri/icons/"
done
cp -p "$root/src-tauri/com.kadaken.wayfinder.metainfo.xml" "$top/src-tauri/"
cp -p "$root/README.md" "$root/LICENSE" "$root/CHANGELOG.md" "$root/THIRD-PARTY-NOTICES.md" \
   "$root/SECURITY.md" "$root/CONTRIBUTING.md" "$top/"
chmod 0755 "$top/packaging/install.sh" "$top/packaging/dependencies.sh" \
            "$top/scripts/wayfinder-new-window" \
            "$top/src-tauri/target/release/wayfinder-native" \
            "$top/src-tauri/target/release/wayfinder-portal"

cat > "$top/INSTALL.txt" <<TXT
Wayfinder $version, built for Linux $arch.

Install it for yourself, under ~/.local:

    ./packaging/install.sh

Then log out and back in once, so the desktop picks up the file dialog.

Other options:

    ./packaging/install.sh --check          say what is on this machine, install nothing
    ./packaging/install.sh --with-codecs    also install the codecs previews need
    ./packaging/install.sh --system         install for every user, under /usr/local
    ./packaging/install.sh --no-portal      do not become the system file dialog
    ./packaging/install.sh --uninstall      remove everything it installed

The binaries in here are already built, so nothing needs a Rust toolchain.
Source, and what changed in this version: https://github.com/Kadaken/wayfinder

Something is wrong with it, or you want to help: CONTRIBUTING.md.
Something is a security problem: SECURITY.md -- not a public issue, please.
TXT

mkdir -p "$out"
tarball="$out/$name.tar.gz"
rm -f "$tarball"
tar --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$(git -C "$root" log -1 --format=%ct)" \
    -czf "$tarball" -C "$stage" "$name"

( cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS.txt )
printf '%s\n' "$tarball"
printf '%s\n' "$(cd "$out" && cat SHA256SUMS.txt)"
printf 'size: %s bytes\n' "$(stat -c %s "$tarball")"
