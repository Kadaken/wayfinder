# Third-party notices

Wayfinder itself is GPL-3.0-or-later (see LICENSE). This file lists the
other people's work it is built on, and under what terms.

## System libraries, linked at run time

| Library | Licence |
| --- | --- |
| GTK 4 | LGPL-2.1-or-later |
| libadwaita | LGPL-2.1-or-later |
| GLib / GIO / GObject | LGPL-2.1-or-later |
| GdkPixbuf | LGPL-2.1-or-later |
| GStreamer | LGPL-2.1-or-later |
| Cairo | LGPL-2.1-or-later or MPL-1.1 |
| Pango | LGPL-2.1-or-later |

Each is used as a shared library through its public interface; none of its
source is included here. Their own copies live with your distribution's
packages, normally under /usr/share/licenses.

## External programs, called as commands

`xdg-open`, `xdg-mime`, `lsblk`, `udisksctl`, `zip`, `cp`, `kill` and
`fusermount`. They are run as separate programs, never linked in, and each
carries its own licence from your distribution.

## Rust crates compiled in (168)

| Crate | Version | Licence |
| --- | --- | --- |
| async-broadcast | 0.7.2 | MIT OR Apache-2.0 |
| async-channel | 2.5.0 | Apache-2.0 OR MIT |
| async-executor | 1.14.1 | Apache-2.0 OR MIT |
| async-io | 2.6.0 | Apache-2.0 OR MIT |
| async-lock | 3.5.0 | Apache-2.0 OR MIT |
| async-process | 2.5.0 | Apache-2.0 OR MIT |
| async-recursion | 1.2.0 | MIT OR Apache-2.0 |
| async-signal | 0.2.14 | Apache-2.0 OR MIT |
| async-task | 4.7.1 | Apache-2.0 OR MIT |
| async-trait | 0.1.92 | MIT OR Apache-2.0 |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT |
| atomic_refcell | 0.1.14 | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| blocking | 1.7.0 | Apache-2.0 OR MIT |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| cairo-rs | 0.22.0 | MIT |
| cairo-sys-rs | 0.22.0 | MIT |
| cfg-expr | 0.20.8 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| concurrent-queue | 2.5.0 | Apache-2.0 OR MIT |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| either | 1.18.0 | MIT OR Apache-2.0 |
| endi | 1.1.1 | MIT |
| enumflags2 | 0.7.12 | MIT OR Apache-2.0 |
| enumflags2_derive | 0.7.12 | MIT OR Apache-2.0 |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| event-listener | 5.4.2 | Apache-2.0 OR MIT |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT |
| fastrand | 2.5.0 | Apache-2.0 OR MIT |
| field-offset | 0.3.6 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| futures-channel | 0.3.32 | MIT OR Apache-2.0 |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| futures-executor | 0.3.32 | MIT OR Apache-2.0 |
| futures-io | 0.3.34 | MIT OR Apache-2.0 |
| futures-lite | 2.6.1 | Apache-2.0 OR MIT |
| futures-macro | 0.3.32 | MIT OR Apache-2.0 |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 |
| futures-task | 0.3.32 | MIT OR Apache-2.0 |
| futures-util | 0.3.32 | MIT OR Apache-2.0 |
| gdk-pixbuf | 0.22.0 | MIT |
| gdk-pixbuf-sys | 0.22.0 | MIT |
| gdk4 | 0.11.4 | MIT |
| gdk4-sys | 0.11.4 | MIT |
| gdk4-wayland | 0.11.5 | MIT |
| gdk4-wayland-sys | 0.11.5 | MIT |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| gio | 0.22.8 | MIT |
| gio-sys | 0.22.8 | MIT |
| glib | 0.22.8 | MIT |
| glib-macros | 0.22.6 | MIT |
| glib-sys | 0.22.8 | MIT |
| gobject-sys | 0.22.6 | MIT |
| graphene-rs | 0.22.8 | MIT |
| graphene-sys | 0.22.8 | MIT |
| gsk4 | 0.11.4 | MIT |
| gsk4-sys | 0.11.4 | MIT |
| gstreamer | 0.25.4 | MIT OR Apache-2.0 |
| gstreamer-app | 0.25.2 | MIT OR Apache-2.0 |
| gstreamer-app-sys | 0.25.4 | MIT |
| gstreamer-base | 0.25.4 | MIT OR Apache-2.0 |
| gstreamer-base-sys | 0.25.4 | MIT |
| gstreamer-sys | 0.25.4 | MIT |
| gtk4 | 0.11.4 | MIT |
| gtk4-macros | 0.11.4 | MIT |
| gtk4-sys | 0.11.4 | MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hermit-abi | 0.5.3 | MIT OR Apache-2.0 |
| hex | 0.4.3 | MIT OR Apache-2.0 |
| indexmap | 2.14.0 | Apache-2.0 OR MIT |
| itertools | 0.15.0 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| js-sys | 0.3.106 | MIT OR Apache-2.0 |
| kstring | 2.0.5 | MIT OR Apache-2.0 |
| libadwaita | 0.9.1 | MIT |
| libadwaita-sys | 0.9.1 | MIT |
| libc | 0.2.186 | MIT OR Apache-2.0 |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| log | 0.4.34 | MIT OR Apache-2.0 |
| memchr | 2.8.2 | Unlicense OR MIT |
| memoffset | 0.9.1 | MIT |
| muldiv | 1.0.1 | MIT |
| nix | 0.31.3 | MIT |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| num-rational | 0.4.2 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| option-operations | 0.6.1 | MIT/Apache-2.0 |
| ordered-stream | 0.2.0 | MIT OR Apache-2.0 |
| pango | 0.22.8 | MIT |
| pango-sys | 0.22.0 | MIT |
| parking | 2.2.1 | Apache-2.0 OR MIT |
| pastey | 0.2.3 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| piper | 0.2.5 | MIT OR Apache-2.0 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| polling | 3.11.0 | Apache-2.0 OR MIT |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.106 | MIT OR Apache-2.0 |
| quote | 1.0.45 | MIT OR Apache-2.0 |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.228 | MIT OR Apache-2.0 |
| serde_core | 1.0.228 | MIT OR Apache-2.0 |
| serde_derive | 1.0.228 | MIT OR Apache-2.0 |
| serde_json | 1.0.150 | MIT OR Apache-2.0 |
| serde_repr | 0.1.21 | MIT OR Apache-2.0 |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| slab | 0.4.12 | MIT |
| smallvec | 1.15.2 | MIT OR Apache-2.0 |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 |
| syn | 2.0.118 | MIT OR Apache-2.0 |
| syn | 3.0.6 | MIT OR Apache-2.0 |
| system-deps | 7.0.8 | MIT OR Apache-2.0 |
| system-deps | 9.0.0 | MIT OR Apache-2.0 |
| target-lexicon | 0.13.5 | Apache-2.0 WITH LLVM-exception |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| toml | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_edit | 0.25.12+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_parser | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_writer | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| tracing | 0.1.44 | MIT |
| tracing-attributes | 0.1.31 | MIT |
| tracing-core | 0.1.36 | MIT |
| typed-path | 0.13.0 | MIT OR Apache-2.0 |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| uds_windows | 1.2.1 | MIT |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| uuid | 1.27.0 | Apache-2.0 OR MIT |
| version-compare | 0.2.1 | MIT |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| wasm-bindgen | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.129 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| winnow | 1.0.3 | MIT |
| zbus | 5.19.0 | MIT |
| zbus_macros | 5.19.0 | MIT |
| zbus_names | 4.3.5 | MIT |
| zcheapstr | 1.1.0 | MIT |
| zip | 9.3.0 | MIT |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.21 | MIT |
| zopfli | 0.8.3 | Apache-2.0 |
| zvariant | 5.15.1 | MIT |
| zvariant_derive | 5.15.1 | MIT |
| zvariant_utils | 4.2.0 | MIT |
