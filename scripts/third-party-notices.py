#!/usr/bin/env python3
"""Regenerates the crate table in THIRD-PARTY-NOTICES.md from Cargo.lock.

The licence of each crate is read from the copy in the local cargo registry,
so nothing is guessed and nothing is fetched. Run it after any dependency
change:

    python3 scripts/third-party-notices.py

Only the crate table is rewritten; everything above it is left alone.

SPDX-License-Identifier: GPL-3.0-or-later
"""
import glob
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
lock = (root / "src-tauri" / "Cargo.lock").read_text()
notices = root / "THIRD-PARTY-NOTICES.md"

crates = []
for block in lock.split("[[package]]")[1:]:
    name = re.search(r'^name = "(.+)"$', block, re.M)
    version = re.search(r'^version = "(.+)"$', block, re.M)
    if not name or not version:
        continue
    if name.group(1) == "wayfinder":
        continue
    crates.append((name.group(1), version.group(1)))

# Crates in the lock file that this machine never downloaded, because they are
# only built on another platform. Their licence is taken from crates.io and
# written down here rather than guessed; check with `cargo info <crate>`.
known = {"nix": "MIT", "cfg_aliases": "MIT"}

missing = []
rows = []
for name, version in sorted(crates):
    licence = None
    for manifest in glob.glob(
        str(pathlib.Path.home() / f".cargo/registry/src/*/{name}-{version}/Cargo.toml")
    ):
        text = pathlib.Path(manifest).read_text()
        found = re.search(r'^license = "(.+)"$', text, re.M)
        if found:
            licence = found.group(1)
            break
        if re.search(r"^license-file = ", text, re.M):
            licence = "see the crate's own licence file"
            break
    if licence is None:
        licence = known.get(name)
    if licence is None:
        licence = "UNKNOWN"
        missing.append(f"{name} {version}")
    rows.append(f"| {name} | {version} | {licence} |")

table = [
    f"## Rust crates compiled in ({len(rows)})",
    "",
    "| Crate | Version | Licence |",
    "| --- | --- | --- |",
    *rows,
    "",
]

text = notices.read_text()
head = text[: text.index("## Rust crates compiled in")]
notices.write_text(head + "\n".join(table))

print(f"{len(rows)} crates written")
if missing:
    print("licence not found for: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
