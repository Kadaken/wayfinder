#!/usr/bin/env bash
# Print the rustc flags that keep the build machine out of the binaries.
#
# Without them, every panic string and `track_caller` path a dependency
# compiles in carries the absolute path it was built from -- 151 copies of the
# build user's home folder in wayfinder-native and 112 in wayfinder-portal,
# readable with `strings` by anyone who downloads the archive.
#
# `trim-paths` in Cargo.toml would be the tidy way to say this, but it is still
# unstable in Cargo 1.96, so the same thing is said with --remap-path-prefix.
# Nothing machine-specific is written down here: the paths are read from the
# environment at build time, which is also why this is a script and not a
# committed .cargo/config.toml.
#
# The flags come out separated by the unit separator (0x1f), which is what
# CARGO_ENCODED_RUSTFLAGS takes. Plain RUSTFLAGS is split on spaces, so it
# would mangle any path with a space in it -- and this project lives in one.
#
#   CARGO_ENCODED_RUSTFLAGS="$(scripts/release-rustflags.sh)" \
#     cargo build --release --bins
#
# Use the SAME value for `cargo test`, or the two builds fight over one target
# directory and each invalidates the other.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"

us=$'\x1f'
printf '%s' "--remap-path-prefix=$cargo_home=/cargo"
printf '%s%s' "$us" "--remap-path-prefix=$rustup_home=/rust"
printf '%s%s' "$us" "--remap-path-prefix=$root=/wayfinder"
