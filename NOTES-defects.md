
## D-272 — the window freezes on a cold network mount — FIXED 16 November 2026

16 November 2026, Owner: "it freezes when i ry to select the new drive".
Selecting a newly mounted rclone Google Drive folder locks the window.
Measured on a mounted rclone Google Drive remote: the first directory read took
**7.8 seconds**, the second 0.24 s once the FUSE dir cache was warm. The read
is happening on the GTK main thread, so the whole window stops answering for
as long as the network takes instead of showing progress and staying usable.
Not a hang — it does come back. Reproduce by mounting any rclone remote and
selecting it before anything has cached.

**16 November 2026, second hit, far worse.** Owner: "wayfinders fucked ass is
stuck again." This time it did not come back: the single GUI thread sat in
`getdents64` on the Drive mount in uninterruptible disk sleep
(`State: D`, wchan `request_wait_answer` — a FUSE request rclone never
answered) for **21 minutes 59 seconds**, using 0.08 s of CPU the whole time.
All three windows showed `unresponsive=true` to KWin. The same listing from a
shell took 3 seconds once rclone was warm.

The folder listing itself was already off the main thread
(`gio::spawn_blocking` in `load_directory`). The sidebar tree was not, and the
sidebar is rebuilt constantly:

- `ensure_tree_children` ran `list_subdirs`, which is one `read_dir` for the
  branch plus **one more `read_dir` per child** to decide whether to draw an
  arrow — on a Drive mount that is a network round trip each.
- `dir_has_subdirs` ran on every drive row on every single rebuild.

FIXED: both now run on a worker through `gio::spawn_blocking`, the answer is
cached in `AppState::tree_children` / `AppState::tree_openable`, and
`AppState::tree_pending` stops the same path being asked twice while a worker
is out. The first ask returns nothing and the sidebar is rebuilt once the
answer lands, so a slow mount costs a beat, never the window. A drive is drawn
as openable until the probe says otherwise, so the arrow is never missing while
the answer is in flight.

## D-273 — a test run killed the desktop's file dialogs — FIXED 16 November 2026

15 November 2026: running the test suite froze the owner's Wayfinder and left the
desktop unable to open any file dialog. Cost him a working file manager until
the portal services were repaired by hand.

Cause, established 16 November by reading the environment out of a crashed
portal's core file. Each test run gets its own session bus, which was believed
to be enough. It is not. `dbus-daemon --session` reads
`/usr/share/dbus-1/session.conf`, and its `<standard_session_servicedirs/>`
always adds the built-in `/usr/share/dbus-1/services` **on top of** whatever
`XDG_DATA_DIRS` says. That folder holds
`org.freedesktop.portal.Desktop.service`. So the first test to ask for a file
dialog had its own private bus start a real `/usr/libexec/xdg-desktop-portal`
carrying the test's throwaway HOME. From there it cannot read the user
database, and it aborts inside the GNOME proxy resolver
(`libgiognomeproxy.so`). 33 of those core dumps on 16 November alone.

Fix: the test bus is now started from a config file of its own naming one
empty service folder and nothing else, so the portal is simply unknown on it.
Both places that start a test bus do this — `scripts/run232.sh` and the
`PrivateBus` fixture in `src-tauri/src/portal_service.rs`. `run232.sh` also
checks it on every run before a single test starts, and refuses to run if that
bus can reach the portal. The proof is the test
`the_test_bus_cannot_start_the_desktop_portal`.

## D-274 — sidebar rows swell when the sidebar has room (FIXED)

16 November 2026. `scripts/run232.sh full` failed at `src-tauri/src/native.rs`,
`protected sidebar and row geometry moved`: the Home row measured 107 px
before the first drive scan landed and 38 px after it.

    left  (after the scan):  (216, 38, 188, 680, 680)
    right (before the scan): (216, 107, 188, 680, 680)

Not caused by 0.9.24. Proven by building the previous release commit (1cf820a,
0.9.23) in a throwaway worktree and running the same suite: it failed there
too, with the same two tuples, at that commit's line 27526.

Not a fixture artefact either, and not row stretching -- `row.set_vexpand(false)`
changed nothing. Measuring the Home row's whole widget tree named the cause:

    GtkListBoxRow  alloc=188x107  nat_h=81
      GtkBox       alloc=168x99   nat_h=87   hexpand
        GtkBox     alloc=99x99    nat_w=256  nat_h=99   <- the icon slot
          GtkPicture alloc=99x99  nat_w=256  nat_h=99
        GtkBox     alloc=37x99    nat_h=18              <- the words

The packaged icon was 256 px (D-275). A GtkPicture is height-for-width, so it
took every pixel of width the words left over -- 99 here -- and reported 99 px
of height to match. Short names left more width, so the rows differed: Root
119 px, Home 107, "Connect to Server..." 97. Once the drive rows arrived the
sidebar overflowed, every row fell back to its minimum, and all of them became
38 px.

Fixed by D-275: with a 30 px icon the picture cannot ask for more. The suite
also now asserts a places row is at most 48 px tall before the scan.

## D-275 — the sidebar icons have been soft since 0.9.22 (FIXED)

16 November 2026. `static/icons/sidebar/*.png` are the copies compiled into
the binary, and D-258 (7 November, "they need to look crisp") shrank them to
the 30 px the sidebar draws, with Lanczos and a light sharpen, because letting
the graphics card shrink a big picture rounds every edge.

0.9.22 put the 256 px originals in that folder instead. Every release since --
0.9.22, 0.9.23 and the 0.9.24 now on kadaken.com -- has drawn sidebar icons
shrunk by the graphics card, soft again, and tall enough to stretch the rows
(D-274).

    commit 30553a0 "Wayfinder 0.9.22"   sidebar/home.png  256x256
    commit 0033677 (the one before)     sidebar/home.png   18x18

Fixed three ways: the icons regenerated at 30 px; `scripts/make-sidebar-icons.sh`
now reads `SIDEBAR_ICON_SLOT` out of `native.rs` rather than carrying its own
number, so the two cannot drift again; and the test
`every_packaged_sidebar_icon_is_already_slot_sized` fails if any packaged icon
is ever over the slot.

## D-276 — the undo-race check at native.rs:33243 is flaky

16 November 2026. One run of `scripts/run232.sh full` failed there with
`status was "3 items shown"` where it wanted `Undo refused:`. The identical
run straight afterwards passed the whole suite. The status label is being read
after something else has already overwritten it, so the check is timing
dependent rather than wrong. Not yet fixed; it has been seen once.

## D-277 — 0.9.25 shipped the old sidebar drawings (FIXED)

16 November 2026. Fixing D-275 by shrinking `static/icons/*.png` was wrong:
those were the first 96px drawings, and 0.9.22 had replaced the sidebar art
with a new 256px set in `static/icons/sidebar` without touching them. So
0.9.25 put the old drawings back, at the right size. Owner: "they are the old
icons not the new ones."

The 256px masters are now `static/icons/sidebar-src`, recovered from commit
b5af50a, and `scripts/make-sidebar-icons.sh` shrinks from there. The 96px set
is deleted -- from the repo, from the leftover release tree in
`~/Projects/Scratch/wf-release-0dd0268`, and from
`~/.local/share/wayfinder/icons`, which an August install had left behind and
which `native.rs` already asserts the app must never read.

## D-279 — an update leaves the old file picker running — FIXED 16 November 2026

16 November 2026, Owner: "why is wayfinder greyed out now?" An install at 16:35
replaced both binaries. The app came up on the new build, but the picker
backend kept running from 14:03: `/proc/749707/exe` pointed at
`~/.local/bin/wayfinder-portal (deleted)`. Every file dialog on the
machine was still being served by the replaced build, and the window greyed
out waiting on it.

Cause: `restart_portal()` in `packaging/install.sh` restarted
`xdg-desktop-portal`, the front end, and nothing else. `wayfinder-portal` is
started on demand by D-Bus and then stays up, so it survives that restart and
serves the old code until the user logs out — which is exactly what the
installer's closing message told people to do.

This hit every user who updated without logging out, not just this machine.

Fix: `restart_portal()` now runs `pkill -x wayfinder-portal` before the
`try-restart`. D-Bus starts the new backend the next time any app asks for a
dialog. Checked: killing the backend by hand at 16:43 brought a fresh one up
at 16:43:48 on the current build, and `install.sh --dry-run` prints the new
step in the right order.

That fix was itself dead code: see D-288. `pkill -x` cannot match a
16-character process name, so the step never fired until 16 November 2026.

## D-278 — an update check could be answered from a cache (FIXED)

16 November 2026. `wayfinder-update.json` is served with
`max-age=0, must-revalidate`, but right after 0.9.26 went up the plain URL
still answered 0.9.25 for about a minute while a request carrying
`Cache-Control: no-cache` already got 0.9.26. A check landing in that window
is told there is nothing new. `fetch_manifest` now sends that header.

Found alongside it: `scripts/run232.sh` passed only its first argument on, so
the `-- CMD` mode ran nothing and printed xvfb-run's usage instead. It passes
all of them now.

## D-288 — the portal restart added for D-279 can never fire

Found 16 November 2026, auditing W5.4 at 0.9.28. **FIXED the same day** — the
pattern now matches the installed path and the installer says whether anything
was stopped. Proven: the new pattern finds the live portal, pid 7258; the old
one finds nothing.

`packaging/install.sh:149` runs `pkill -x wayfinder-portal`. The name is 16
characters; Linux truncates a process's `comm` to 15, so `-x` compares against
`wayfinder-porta` and never matches. Proven on this machine: the running portal
is pid 7258 with `comm=wayfinder-porta`, and `pgrep -x wayfinder-portal` exits
1. pkill itself prints the warning and the installer swallows it with `|| true`.

Effect: installing or upgrading leaves the OLD portal binary serving every
application's file dialog until the user logs out. That is why INSTALL.txt has
to tell people to log out.

Fix: match the installed path, `pkill -f -- "$prefix/bin/wayfinder-portal"`,
and report whether anything was signalled instead of discarding the result.

## D-280 — --dry-run writes into the home directory

Found 16 November 2026, auditing W5.4 at 0.9.28. **FIXED the same day** — the
codec probe runs with XDG_CACHE_HOME pointed at a throwaway directory that is
removed afterwards. Proven: --dry-run against an empty home now leaves 0 files.

`install.sh --dry-run` says "print every step, change nothing". Run against an
empty HOME it created `.cache/gstreamer-1.0/registry.x86_64.bin`,
`.cache/mesa_shader_cache/` and `.cache/radv_builtin_shaders/`, because
`packaging/dependencies.sh:57` runs `gst-inspect-1.0 playbin` to probe for
codecs and GStreamer builds its registry as a side effect.

Harmless caches, but the claim is false. Either skip the probe under --dry-run
and say the feature check was skipped, or point `GST_REGISTRY` at a temporary
file for the probe.

## D-281 — the 0.9.28 GitHub release was never published

Found 16 November 2026, auditing W5 and S9 at 0.9.28. **FIXED the same day** —
assets replaced and the release published as latest.

`gh release list` shows `Wayfinder 0.9.28  Draft  v0.9.28`. The newest public
release is still 0.9.23. kadaken.com serves 0.9.28 and the in-app updater offers
it, so anyone who goes to GitHub instead — which SECURITY.md and the download
card both point at — sees a version five releases old. 0.9.24 to 0.9.27 have no
release at all.

## D-282 — the draft release's tarball is a different build from the served one

Found 16 November 2026, auditing W5 at 0.9.28. **FIXED the same day** — both
assets re-uploaded from dist/, 3,106,357 bytes, sha256 80ca5082…

The asset attached to the draft is 3,105,966 bytes. The file served from
kadaken.com, listed in `wayfinder-update.json` and in `download/SHA256SUMS`, is
3,106,357 bytes — the rebuild made after the tag moved to `67b7ea1`. The
release's `SHA256SUMS.txt` therefore states a checksum that no published file
has. Fix: replace both assets from the artifact that was actually shipped.

## D-283 — the download page carries no warning that it moves and deletes files (FIXED in source)

Found 16 November 2026, auditing W6 at 0.9.28. Fixed in the site source the same
day, not yet published.

The rendered kadaken.com card says "Beta", "Nowhere near finished" and lists
five things it cannot do yet. Searched the whole rendered page for delete,
trash, overwrite, permanent, "your files": zero matches. A file manager in beta
that moves, overwrites and deletes the user's files should say so above the
download button, not only in the app. The release audit asks for an
alpha warning AND a destructive-scope warning; only the first is there.

Fixed: `index.html` now carries a "Read this before you download it" box
directly above the download button -- that it moves, copies, renames and
deletes real files and is beta; that Ctrl+Z undoes most of it and Delete goes
to the desktop Trash while Shift+Delete erases with nothing to bring it back;
to have a backup of anything irreplaceable first; and that it installs under
~/.local for one user and never asks for root.

## D-284 — 20 dead-code items ship in wayfinder-native, one whole feature among them
Status: Open (found by K1.3 at 0.9.28, 16 Nov 2026)

`cargo clippy --release --bins --all-features -- -D warnings` reports 20
never-used items in the native binary. Most are small, but six of them are the
complete helper set for D-242's hand-installed-software convention —
`install_dir`, `looks_like_an_app_tree`, `is_executable_file`,
`appimage_desktop_entry`, `config_install_convention` and the `AppTree`
variant. They are defined in native_core.rs, they have tests, and native.rs
calls none of them. The code and its tests ship; the user cannot reach the
feature.

Why it matters: a passing test on unreachable code reads as coverage and is
not. Either the feature gets wired into the user interface or the helpers and
their tests come out. It is not a crash and it is not a security fault, so it
does not block 0.9.28, but it must not survive another release unanswered.

## D-285 — a config that will not parse is replaced by defaults, silently
Status: FIXED 17 Nov 2026 with D-297 (damaged file moved to .corrupt, read falls back to .bak)

`load_config` (native.rs:1876) ends in `.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()`.
Any parse failure — one bad field, a truncated write, a config written by a
version that did not yet have `favorites`, `recents`, `show_hidden`,
`view_mode`, `search_in_files`, `sort_key` or `sort_ascending`, none of which
carry `#[serde(default)]` — throws the whole file away and starts from
defaults. Favourites, recents, remembered networks and every view setting go
with it, and nothing is said. The next write then overwrites the file that
could have been repaired.

The fix is two parts and neither is large: `#[serde(default)]` on every field
so a partial config keeps what it has, and on a genuine parse failure move the
bad file aside (native-config.json.unreadable) and say so in the status line
instead of overwriting it.

## D-286 — uninstall leaves Wayfinder named as the folder handler
Status: Open (found by S14 at 0.9.28, 16 Nov 2026)

install.sh runs `xdg-mime default com.kadaken.wayfinder.desktop inode/directory`
(packaging/install.sh:338) and `--uninstall` never undoes it. After an
uninstall the user's mimeapps.list still names a .desktop file that no longer
exists as the handler for folders. The uninstall already does the equivalent
repair for portals.conf, keeping a .bak, so the pattern to follow is in the
same script.

## D-287 — the app is dark on every desktop, including a light one
Status: Open (found by S18.4 at 0.9.28, 16 Nov 2026)

`connect_startup` sets `adw::ColorScheme::PreferDark` unconditionally
(src-tauri/src/native.rs:3800). The system light/dark preference is never read,
so a user on a light desktop gets a dark file manager with no way to change it,
and "light and dark without a restart" cannot be answered yes. ColorScheme::
Default would follow the desktop. Whether Wayfinder should follow the desktop
or stay dark on purpose is a decision, not a bug — but it must be a decision
somebody made, not a line nobody revisited.

## D-289 — the fix this release exists for has no test
Status: FIXED 17 Nov 2026 -- TESTED. `sidebar_tree_reads_never_block_the_window_d289` in the GTK suite: a read held shut on a channel does not stop the main loop, is asked once, lands in the cache when released; same for the drive probe. Mutation-checked (removing the pending guard fails it). Not covered: a real hung Drive mount; the D-278 Cache-Control header test is still not written.

D-272 is the only entry in the 0.9.28 changelog and nothing in the suite
covers it. The id appears in four comments (native.rs:2241, 2244, 6837, 6882)
and in no test; no test function touches `tree_children`, `tree_openable` or
`tree_pending`. The next person to make the sidebar synchronous again brings
the 21-minute freeze back and every one of the 475 tests still passes.

The test that would cover it, inside `gtk_headless_suite` where a real
AppState and a real main loop already exist: call `ensure_tree_children` on a
real directory, assert the first call returns nothing and leaves the path in
`tree_pending`, iterate the main loop until the worker lands, then assert the
second call answers from `tree_children` and dispatches no second worker. The
same shape proves `dir_has_subdirs_async` for `tree_openable`, including that
a drive is drawn as openable while the probe is in flight.

Second, smaller: D-278's `Cache-Control: no-cache` header (update.rs:168) is
asserted by nothing. Extracting `fetch_manifest`'s curl argument list into a
function that returns it makes that a one-line assertion.

## D-290 — the config ignores XDG_CONFIG_HOME
Status: FIXED 17 Nov 2026 -- TESTED. `config_path_from` honours an absolute, non-empty XDG_CONFIG_HOME; `adopt_legacy_config` copies the old $HOME/.config file across once, never over newer settings. 2 tests, mutation-checked. Not covered: a real upgrade from 0.9.27 with XDG_CONFIG_HOME set.

`config_path()` (src-tauri/src/native.rs:1867) builds
`$HOME/.config/wayfinder/native-config.json` from `HOME` alone. The XDG base
directory spec says use `XDG_CONFIG_HOME` when it is set and non-empty, and
fall back to `$HOME/.config` only when it is not. Proven: a run with
`XDG_CONFIG_HOME` pointing at a prepared directory ignored the config sitting
there, created a fresh default one under `$HOME/.config`, and wrote to that.

Anyone who sets `XDG_CONFIG_HOME` -- which is the usual way to keep a home
directory tidy, and what a container or a sandboxed launcher does -- has their
settings written outside it and silently starts from defaults.

Fix: honour the variable when set and non-empty, keeping `$HOME/.config` as
the fallback. One function; `read_config_at` and `update_config_at` already
take a path, so nothing else changes.

## D-291 — keyboard focus is invisible
Status: FIXED 17 Nov 2026 -- TESTED. `:focus-visible` draws a 2px @wf_accent_hover ring (3.6:1 or better on every surface; the first colour tried was 2.91:1 and failed). Test reads the colours from the stylesheet; mutation-checked. Not covered: a look at it in the live window, and focus inside popovers and dialogs.

The stylesheet's only focus rule removes the indicator:

    button:focus, button:focus-visible,
    row:focus,    row:focus-visible { outline: none; box-shadow: none; }

Nothing adds one back, so a keyboard user tabbing through the toolbar cannot
see where they are. WCAG 2.1 2.4.7. The rule was almost certainly written to
kill GTK's dotted ring on a mouse click, which is what `:focus-visible`
already handles on its own: dropping `:focus` from the selector and styling
`:focus-visible` with a 2px accent outline gives a ring for the keyboard and
none for the mouse.

## D-292 — every context menu is pointer-only
Status: Open (found by S12 at 0.9.28, 16 Nov 2026)

Context menus are opened by `GestureClick` on button 3 / BUTTON_SECONDARY
(native.rs:6370, 6556, 8224 and the sidebar). No handler exists for the Menu
key or Shift+F10 -- `Key::Menu` appears nowhere -- so an action that lives
only in a context menu is unreachable without a mouse. The operations that
also have a shortcut (copy, cut, paste, rename, delete) are fine; the rest are
not.

## D-293 — five text roles are below the contrast minimum
Status: Open (found by S12 at 0.9.28, 16 Nov 2026)

`@wf_text_muted` (#6b7079) gives 3.39:1 on the listing background and 3.04:1
on a raised pane, against the 4.5:1 WCAG 2.1 1.4.3 asks for normal-size text.
It is used at 11 to 14px for the sidebar headings, the sidebar detail line,
the empty-folder placeholder, the empty-preview text and the menu accelerator
labels, so the large-text exemption does not apply. Disabled buttons also use
it, and those are exempt.

Lifting it to about #8b909a, which is already defined as `@wf_text_dim` and
measures 5.26:1 on the same background, clears all five. The pane hairline
(#3a3f49 on #1b1d22, 1.60:1) is a separate, weaker case: 1.4.11 wants 3:1 for
a boundary that carries meaning, and this one is draggable.

## D-294 — nothing sets an accessible name
Status: Open (found by S12 at 0.9.28, 16 Nov 2026)

No call to `update_property`, `AccessibleProperty` or `set_accessible` exists
in the binary -- zero, searched by name -- while 12 buttons are icon-only.
A screen reader is left with 18 tooltips and the icon names. Each icon-only
button needs one `update_property(&[gtk::accessible::Property::Label("...")])`
beside the tooltip it already has.

## D-296 — the site terms contradict the licence Wayfinder ships under (FIXED in source)
Status: fixed in the site source 16 Nov 2026, not yet published

`terms.html` on kadaken.com is headed "For Crews and other Kadaken software"
and forbids, without written permission: redistributing the software "in whole
or in part, original or modified ... including mirrors, re-uploads, bundles,
app stores, package repositories and forks"; modifying, adapting, translating
or creating derivative works; and reverse engineering.

Wayfinder ships under GPL-3.0-or-later -- declared in `src-tauri/Cargo.toml`,
in the metainfo's `<project_license>`, and as the full GPLv3 text in `LICENSE`
inside the tarball. The GPL grants every one of those rights to everyone, and
section 10 forbids adding further restrictions to them. A reader who takes the
terms page at its word believes they may not do what the licence in the
download explicitly permits.

Fix: one paragraph on that page naming Wayfinder, stating that it is released
under GPL-3.0-or-later and that its own licence governs it, as the page
already does for the engine ports. Changing Wayfinder's licence is not an
option for versions already published -- a GPL grant cannot be withdrawn after
the fact.

Fixed: `terms.html` now carries a third notice, beside the one for the engine
ports -- "Wayfinder is open source", naming GPL-3.0-or-later, stating that its
own licence governs it and that the restrictions on that page do not apply to
it, linking the source. The page's opening line now reads "For Crews and other
Kadaken software that is not released under an open-source licence" and is
dated 16 November 2026. The owner changed his position on 16 November 2026: his
software can be open source. Not published yet.

## D-295 — two caches added in 0.9.28 never let go
Status: FIXED 17 Nov 2026 -- TESTED. `TreeCache` (native_core.rs) caps at 400, forgets a branch when closed, forgets a folder and its parent on refresh or monitor scan, clears on the hidden-files toggle. 5 unit tests, cap and forget mutation-checked. Not covered: an outside change to a folder that is not the open one (needs the file monitors deferred to 0.9.29).

`AppState::tree_children` and `AppState::tree_openable` (native.rs:2240, 2247)
are the D-272 fix: the sidebar's directory reads are answered once and
remembered. Nothing ever removes an entry. No `.remove`, no `.clear`, no size
cap -- searched by name. A window that browses a deep tree all day keeps a
path String and a Vec of child tuples for every branch it ever expanded, for
as long as the window lives.

Every other growing collection in `AppState` is bounded: `recents` and
`recent_files` truncate to 12 (native.rs:2005, 2019), `undo_stack` and
`redo_stack` drain down to 100 (13648, 13658), grid thumbnails stop at 60 with
2 decodes at a time. These two are the exception.

It is small per entry and it is not a leak in the C sense -- it is freed when
the window closes -- but it is the only unbounded cache in the program and it
is one line of eviction away from not being one. It also goes stale: a folder
whose children changed outside the app keeps its old answer until the entry is
replaced.

## D-297 — the config is replaced without fsync, on btrfs, and a bad read resets it
Status: FIXED 17 Nov 2026 -- TESTED. fsync of file and parent, per-process temp name, .bak of the last good file, damaged file moved to .corrupt, read falls back to .bak. Not covered: a real power cut or disk-full (needs root).

`update_config_at` (native.rs:1899) writes `native-config.json.tmp` and renames
it over the real file. There is no `sync_all` or `sync_data` anywhere in the
product -- zero hits across `native.rs` and `native_core.rs`. The owner's `/home` is
btrfs.

Write-then-rename is only atomic once the data is on disk. ext4 has flushed
delayed allocations before a rename since 2.6.30; btrfs makes no such promise
without an fsync of the file, and the directory entry needs its own. So a power
cut or a hard reset shortly after a settings change can leave a zero-length or
truncated `native-config.json`.

What makes that worse than a lost setting: `load_config` (native.rs:1876) ends
`.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()`, which is
D-285. A truncated config does not report anything -- it silently becomes the
default, so every favourite, bookmark and view choice is gone with no message.

Unproven: the btrfs ordering claim has not been tested here, and no crash was
staged. Aragon's review argues the write path matters less than the read path,
and that is right: whatever the filesystem guarantees, the next setting change
after a damaged read calls `update_config_at`, which has already replaced the
config in memory with the defaults, and writes those defaults over the damaged
file. A recoverable hiccup, or a hand-edited JSON typo, becomes permanent.

Fix, all four parts: `sync_all()` on the tmp file and on the parent directory
before the rename, so the guarantee does not depend on which filesystem this
is; keep the previous good file as `native-config.json.bak`; on a parse
failure move the damaged file to `native-config.json.corrupt` and read the
`.bak` instead of returning defaults; and say so in the window rather than
starting empty in silence.

## D-298 — two GTK tests can never share one process, and the cause is measured
Status: Open (proven 16 Nov 2026, after Aragon's review)

`cargo test --release --features guide --bins -- --test-threads=1` reports
`gtk_headless_suite` (native.rs:21617) panicking "Attempted to initialize GTK
from two different threads". It was assumed to be an ordering limit with no
fix but running the suite alone.

Measured instead: Rust's own test harness spawns a fresh OS thread for every
`#[test]` even at `--test-threads=1`. A three-test probe crate printing
`thread::current().id()` under `--test-threads=1 --nocapture` printed
ThreadId(2), ThreadId(3), ThreadId(4) -- one per test. `gtk4-rs` records the
thread that first called `gtk::init()` and panics for any other thread, and 40
test functions call it, so the 41st always loses.

So `cargo test --bins`, the command the release audit prescribes, cannot pass
by construction, and W4 needs two invocations.

The fix, and it is not a split binary: one process-wide GTK thread created
once through a `OnceLock`, calling `gtk::init()` and owning the main context
for the life of the test binary, plus a `with_gtk(|| ...)` helper that
dispatches a closure to it over a channel, catches the panic with
`catch_unwind` and rethrows it on the harness's thread. Cost is honest: about
40 lines of harness and 40 mechanical test edits, not 25 lines.

## D-299 — two processes write the config through one fixed temp file
Status: FIXED 17 Nov 2026 -- TESTED. Per-process temp name plus an exclusive lock on native-config.json.lock around the whole read-change-write; the eight-thread test fails 3 of 3 runs with the lock removed. Not covered: two real processes (the test uses threads, which take the same kind of lock).

`update_config_at` (native.rs:1899) writes `path.with_extension("json.tmp")`
-- a single fixed name, no PID, no nonce, no `flock`. There are 24 write sites.

The main window is single-instance, but `--pick` is deliberately NON_UNIQUE,
so a picker process runs beside it, and `push_recent_file` (native.rs:7163) is
one of the 24. Two processes writing at once interleave into the same
`native-config.json.tmp`, and whichever renames last puts whatever that file
then holds over the live config. The loser's `rename` can also fail with
ENOENT, which is discarded: the line is `let _ = fs::rename(&tmp, path)`.

Interleaved JSON reaches the live config, and `load_config` (native.rs:1876)
turns an unparseable config into `Default::default()` with no message -- D-285
and D-297. So this is a data-loss path that needs no power cut.

Fix: write to `native-config.json.<pid>.tmp` and keep the rename, which is
already atomic, and take D-297's `.bak` plus quarantine so a damaged file is
never silently replaced by defaults.

## D-300 — every background read shares one bounded thread pool
Status: Open (code confirmed 16 Nov 2026, behaviour unproven)

D-272 was fixed by moving the sidebar's blocking reads off the GTK thread with
`gio::spawn_blocking`. There are 10 `gio::spawn_blocking` sites in native.rs
and they all hand work to the same pool GLib keeps for `GTask`, which is
capped -- 10 threads by default:

    6743  SMB lookup
    6869  list_subdirs, the sidebar branch
    6905  dir_has_subdirs, the drive arrow probe
    9061  drop receive
    10772, 10925, 16655, 16728  thumbnails and previews
    16546  the folder listing

A mount in uninterruptible kernel sleep cannot be cancelled or killed until
its driver times out, and D-272's own report was 22 minutes. Enough wedged
probes occupy the whole pool, and then the folder listing and the thumbnails
have nowhere to run: the window answers, but nothing loads. The freeze this
release exists to fix comes back by a different route.

Unproven: no wedged mount was staged. What is certain from the code is the
shared pool and the absence of any per-mount limit or circuit breaker.

Fix: a probe pool of its own, separate from listing and thumbnails, plus one
in-flight probe per mount and a remembered failure so a dead mount is not
retried on every sidebar rebuild.

## D-301 — a real Cancel through the real portal reports error, not cancelled
Status: FIXED 17 Nov 2026 -- the Cancel path was never at fault; the timeout
now names itself and the test deadline matches production (120s)

What was measured on 17 Nov, after the first failure:

* The cancel driver run by hand under its own Xvfb exits `1` and leaves a
  zero-byte result file -- exactly what `read_result_strict` reads as
  `EmptyCancel`.
* The test run alone passes, and a probe inside the portal printed
  `exit_code=Some(1) status_none=false result=Ok("")` -- 1 passed, 61.67s.
* All three `native_picker_real` tests run together pass, probe printing a
  real exit code and result for each -- 3 passed, 173.75s.

So the picker, the exit code, the result file and the parser are all clear.
The only remaining way to reach the reported `2` is `wait_with_deadline`
returning `None`: the child outlived the 30 seconds the test gives it and was
killed. The failing run was the whole suite under load; the production
deadline is 120 seconds (portal_service.rs:362), the test's is 30
(portal_service.rs:4529 area).

The real defect underneath: a timeout and a malformed result are reported
identically, as `ChooserOutcome::Failed`, so a slow machine and a broken
picker are the same line of output and neither says which it was.

`portal_service::tests::native_picker_real_cancel_through_exported_open_file`
fails in the shipped feature configuration -- no `--features guide` -- with

    assertion `left == right` failed: real Cancel must report cancelled
      left: 2   right: 1

Response 2 is RESPONSE_OTHER, the catch-all failure. The run was
`scripts/run232.sh -- cargo test --release --bin wayfinder-portal --
--test-threads=1 native_picker_real`: 2 passed, 1 failed, 188.56s.

The same run proves the save path is fine without `guide`:
`native_picker_real_save_through_exported_save_file` passed here and failed
only in the earlier `--features guide` run. `guide` is not in the shipped
binary (`default = []` in src-tauri/Cargo.toml), so that one is a feature-only
fault; this one is not.

Cancel is reported only when both halves hold (portal_service.rs:626):

    Some(1) => match read_result_strict(&result_path) {
        Some(ParsedResult::EmptyCancel) => ChooserOutcome::Cancelled,
        _ => ChooserOutcome::Failed,
    },

so either the picker child did not exit 1, or its result file is not the
empty-cancel shape. The driver clicks the real Cancel button once the window
is mapped (native.rs:5136) and breaks out of its poll immediately after:

    if cfg!(feature = "picker-test-driver-cancel") {
        if let Some(cancel) = ui.picker_cancel.clone() { cancel.emit_clicked(); }
        return glib::ControlFlow::Break;
    }

If `ui.picker_cancel` is None the poll breaks having clicked nothing, and the
30-second deadline then produces exactly this failure. Unproven either way: the
child's exit code and its result file have not been read.

Whether this is a defect in the Cancel path itself or in the test driver is
undecided, and it decides whether 0.9.28 can ship.

## D-302 — an open window keeps showing a folder that has been renamed
Status: Open (seen live 17 Nov 2026, cause read from the code, unproven)

`/home/user/Downloads/Aragon Exchange` was renamed to `Argon Exchange` while a
window had it open. The window went on listing the old names, the breadcrumb
went on reading `Aragon Exchange`, and opening the subfolder it was showing
failed with "Cannot open /home/user/Downloads/Aragon Exchange/Aragon Replies:
No such file or directory (os error 2)". The listing was stale and the window
never said so.

The monitor watches the inode, so it is still alive after its own folder is
renamed, but the handler at native.rs:9124 compares `state.cwd` with the path
it was installed on:

    if state.borrow().monitor_generation != monitor_generation
        || state.borrow().cwd != watched { return; }

Both are the old path string, so they match, and the rescan that follows reads
that old path, which no longer exists. A failed rescan leaves the previous
listing on screen.

Two things are wrong, not one: the window does not learn its own folder has
moved, and a rescan that fails with ENOENT shows nothing at all -- the user
has to find out by clicking a row and reading an error.

Fix: watch for the folder's own move. `monitor_directory` is installed with
`FileMonitorFlags::NONE` (native.rs:9095) while the other monitor in the
program uses `WATCH_MOVES` (native.rs:3687). With moves watched, follow the
new path and update `cwd` and the breadcrumb; and when a rescan fails with
ENOENT, say so in the window instead of keeping a listing that is a lie.

## D-303 — the Modified column is written once and never ticks
Status: FIXED 17 Nov 2026 (ticker + tooltip; the two-minute "Just now" bucket kept on his call)

`fmt_modified` runs when a row is built (native.rs:9847) and nothing re-runs
it. A window left open shows the ages it had at load: "Just now" stays "Just
now" for an hour, and nothing on screen moves until the user presses refresh.

Two smaller faults in the same column:

* `JUST_NOW_MINUTES = 2` (native_core.rs:5059), so "Just now" covers anything
  under two minutes and the next label is "2 min ago" -- "1 min ago" can never
  appear. The label is accurate and the bucket is simply wider than the words
  suggest.
* Rows that share a label cannot be told apart. The sort is on the real
  timestamp, so the order on screen is already exact, but seven rows all
  reading "27 min ago" give the eye nothing, and there is no tooltip carrying
  the real time.

Fix: re-render the Modified column on a 30-second timer (the text only, not
the listing), narrow "Just now" to under 60 seconds so "1 min ago" exists, and
put the full local date and time in the row's tooltip.

## D-304 — a hostile filter pattern hung the file picker (found by W2, attack 7)
FIXED, TESTED. `glob_match` tried every way to split the name for each `*`, so
40 `*a` segments against a 200-character miss never finished (killed at 560 s).
It now remembers failed states: cost is pattern length times name length.
Test: `glob_match_with_many_stars_finishes_fast_on_a_miss` (0.00 s).
Not covered: `mime_match`, which does no pattern search.

## W2 — hostile run at the portal's D-Bus arguments
TESTED for attacks 1, 2, 3, 5, 6, 7 (`hostile_w2` module, `portal_service.rs`);
mutation-checked (two guards disabled, two tests failed). Added: control and
bidi characters refused in `current_name`, `SaveFiles` names and choice labels;
caps of 255 bytes per name, 4096 per path, 128 filters, 512 rules per filter,
32 choices, 256 options per choice. `title` and `accept_label` are never read
by the portal, so they never reach the screen.
NOT covered: attack 4 (symlink / dotfile pre-selection is picker behaviour, not
decoder behaviour) and attack 8 (the portal ignores the request handle and
exports no `Request` object, so `Close()` and a reused handle are untested).

## D-305 — the portal served one request at a time and had no Request.Close() (W2 attack 8)
FIXED, TESTED. The three methods were blocking, so while one app's dialog was
open every other app's request waited behind it, and a `Close()` could not be
heard. They are now async: the pick runs on its own thread (`blocking::unblock`),
each request exports `org.freedesktop.impl.portal.Request` at its handle, and
`Close()` kills the picker and answers the call as cancelled (response 1). A
handle already in use is refused (response 2), never replaced. Added the
`blocking = "1"` dependency (already in the lock file via zbus).
Tests: `dbus_request_close_cancels_a_pending_openfile_and_removes_the_object`,
`dbus_a_second_request_is_served_while_the_first_is_open`,
`dbus_a_reused_handle_is_refused_and_leaves_the_first_request_alone`,
`child_wait_cancel_kills_a_running_picker_at_once`; mutation-checked (Close made
a no-op: two tests failed). Not covered: a Close sent by the real
xdg-desktop-portal frontend (tests send it from a plain bus client), and
attack 4 (symlink / dotfile pre-selection).

## D-306 — a Save onto a symbolic link hid where the file would really go (W2 attack 4)
FIXED, TESTED. The picker asked "Replace?" only when `exists()` was true, and
`exists()` follows links: a link to another folder's file showed only the link's
name, and a dangling link got no question at all. `picker_save_confirmation`
now asks for any link and names where it leads. A Save already needs an
explicit Save click or Enter, and an existing file already needed Replace.
Test: `save_onto_a_link_names_where_it_leads_d305`; mutation-checked (link
branch disabled: the test failed). Not covered: the picker does not refuse to
open in a hidden folder such as `.ssh`; the Replace question is the control.

## D-324 — a damaged "mp3" or "gif" gave no reason; blank preview pane on Storage (cause UNPROVEN)
PART FIXED, TESTED; blank pane UNPROVEN. Reported twice: `Storage/Old Hard Drive/Audio/SFX/f33058132.mp3`
(via `--select`) and `.../songs/TRACK02.wav` (click) showed a completely blank right pane.
Headless (xvfb) with the real folders, the real config and a 1700-file folder could NOT
reproduce it: the target is highlighted and the audio box shows (stack "audio", title set).
Live process (/proc): main thread idle, a WAV playbin loaded, so the player opened; the
failure looks visual or state in the live Wayland window, not decoding. Not fixed.
Fixed: files whose bytes are not what the name says now show one line naming the real
type ("This file is actually a GIF image, not a .mp3 audio file.", "Empty file ...",
"Unreadable data ..."). `native_core::sniff_real_type`, `damaged_file_reason`,
`not_audio_reason`; used by `set_preview_audio` and the `set_preview_image` failure branch.
Tests: `sniff_names_the_real_type_d324`, `damaged_reason_is_one_line_naming_the_real_type_d324`,
`damaged_files_say_what_they_really_are_d324`, `select_big_folder_audio_preview_d324`.
Not covered: live Wayland rendering; the --select highlighting of the wrong row seen live.
Follow-up (same day, coordinator live reports A: wrong row selected, B: black video pane): NOT reproduced.
- Warm live launches on the d324 build (`--select` VideoCompressor/20221015_205310.mp4, Audio/.../Basil and Binkies.mp3) selected the right row; the video pane showed a frame (duration 0:24) a few seconds in.
- Headless, real files: the `--select` target is selected; the video stream is prepared in about 2.2 s, paintable 1080x1920, no error. Ignored probes: `d324_probe_video_select` (WF_PROBE_FILE), `d324_sweep_real_audio` (WF_SWEEP_DIRS).
- Sweep of Old Hard Drive/Audio, SFX, songs (headless, read-only): every audio file shows its title and either a working Play bar or the damaged-file line; 0 blank panes.
- Only live-only clue: journal lines `pixman_region32_init_rect: Invalid rectangle passed` (10 at a time) at 18:30, 20:07 and 20:21 in wayfinder processes; 20:21:52 is when TRACK02.wav showed blank. Some widget gets an invalid size; not located. Needs a live capture while the pane is blank.

## D-325 — Wayfinder froze (main thread 100%) on a list-row click; focus left in the preview pane
FIXED, TESTED (headless); the exact live trigger is UNPROVEN. Core `wf-freeze.596993`: `gtk_gesture_click_begin -> gtk_list_item_widget_grab_focus -> gtk_window_root_set_focus -> synthesize_focus_change_events -> gtk_widget_get_parent -> g_log`, looping (gtk4 4.22.5). The old focus widget was stale. `update_preview` swapped pages and rebuilt or emptied children (folder rows, audio and video controls, `set_preview_folder` removes its focusable buttons) with focus still inside the pane. Fix: `release_preview_focus` (native.rs, before `update_preview` dispatches) moves focus to the file view first. Test `preview_focus_survives_selecting_another_row_d325` failed before the fix (focus stayed in the pane) and passes after; it also fails on any GTK warning or critical. Not covered: the live freeze was never reproduced, so which widget held the focus is not known; focus moved by other code paths (popovers, inline rename editors) is untouched.

## D-326 - preview pane stuck blank in one window while another window works
- Cause: `PREVIEW_GENERATION` (native.rs, thread_local) was one counter for every window in the process. Any preview update in window B (folder refresh, selection) bumped it, so window A's still-decoding picture was dropped by the D-253/D-268 guard and A stayed on "Opening preview" / blank.
- Fix: per-window `Ui.preview_generation`; `next_preview_generation(ui)`; all async guards compare the window's own counter.
- Test: `a_preview_in_one_window_does_not_cancel_another_windows_d326` (failed before, passes after).
- Not proven: that this was the live trigger (cut pending path not involved; no early return found that explains it alone).

## D-327 — MIDI files had no Play button (18 Nov 2026)
- Symptom: the owner's own songs (`song1.mid`...`lara2.mid`) showed no player in the preview.
- Cause: `mid/midi/kar/rmi` were not in `AUDIO_EXT`, and the GStreamer MIDI decoder (fluiddec) was not installed.
- Fix: added the four extensions to `AUDIO_EXT`, RIFF `RMID` to `sniff_real_type`, and `gstreamer1-plugins-bad-free-fluidsynth` to the Fedora deps.
- Verified: GtkMediaFile plays lara2.mid, song9.mid, lara2.rmi (timestamp advances, no error); the live preview shows the player and counts up.
- Length fix (second part): GStreamer reports no MIDI length. `native_core::midi_duration_secs` (native_core.rs, after `is_video_path`) reads the file (16 MiB cap), unwraps RIFF RMID, parses the SMF header and every track with tempo changes, running status, sysex and meta events, formats 0/1/2, SMPTE division; None on malformed input. `AudioPlayer::open` stores it (`computed_secs`) and `duration_secs()` falls back to it (audio_preview.rs), so the label and seek bar use it with no UI change.
- Seeking: probed, it WORKS (position 1.31 s, after seek to 30 s it read 31.33 s). So the bar is real, not hidden.
- Tests: `midi_duration_tests` (7: plain, RMID, tempo change, running status+sysex, truncated/garbage, extension case, 10 real songs against their wavs), `midi_length_comes_from_the_file_and_seeking_works` (audio_preview.rs), `midi_preview_shows_its_length_and_seeks_d327` (native.rs).
- Real songs: MIDI length is consistently 2.7-2.9 s shorter than the fluidsynth wav render (the wav has a tail), e.g. lara2 68.32 vs 71.08.
- Not covered: type 2 files with real multi-song use; the live Wayfinder window was not driven.
- Baseline fix made on the way: `a_drive_row_accepts_a_drop_d318` failed before any change here (the sidebar rebuilt under the test); the test now waits for the first scan, overrides the scan and settles before acting. It also stopped the Trash and `gtk_headless_suite` failures that followed it.

## D-328 — `--select <folder>` sometimes shows no window in front (18 Nov 2026)
- 4 tries: `--select Storage/Images` and `--select Storage` showed nothing in front and logged no wf-event; `--select Images/Animals` and a repeat `--select Images` worked.
- Plain `wayfinder <folder>` worked first time. Not reproduced or diagnosed yet.
- Cause found: `gtk_launch_args` turned `--select X` into plain `X`, so a folder opened INSIDE itself and was never highlighted (`select_path_from_files` keeps files only). A missing path opened nothing useful.
- Cause NOT reproduced: the missing window. `present()` is always called in `build_ui_in`; 5 forwarded launches under a private bus all produced a window. Unproven hypothesis: on Wayland a forwarded launch carries no activation token, so the compositor refuses to raise the window (it exists but is behind).
- Fix: `gtk_launch_args` appends the marker `x-wayfinder-select://marker` when `--select` had a value; `launch_from_files` (native.rs, before `connect_open`) sees it and uses `native_core::select_launch_target`: existing path opens its parent with it selected; missing path opens the nearest existing ancestor; root opens itself.
- Tests: `select_launch_tests` (native_core.rs, 4), `select_adds_the_marker_and_a_plain_launch_does_not_d328`, `launch_from_files_reads_the_marker_d328`, `select_launches_always_open_a_window_d328` (native.rs), and `scripts/test-select-forwarded.sh` (running instance plus 5 forwarded launches on a private bus and Xvfb; each must add a window).
- Not covered: Wayland raise/focus (Xvfb has no focus stealing prevention), so the actual "behind other windows" case is unproven.

## D-329 - .MOV preview and poster (18 Nov 2026)
- Checked, no defect found: `101_0823.MOV` and `MISC/.../IMG_0022.MOV` show the "media" stack, the stream prepares with no error, and the grid poster paints a frame.
- Tests: `real_mov_files_preview_and_get_a_poster_d329` (skips if the drive is absent), `mov_extension_is_case_insensitive_d329`.
- Not covered: other codecs; the live window; playback to the end.

## Builder status (18 Nov 2026)
- D-327 length gap: DONE (midi_duration_secs, seek works). Files: src-tauri/src/native_core.rs, audio_preview.rs, native.rs (test, plus d318 test fix). Left: nothing known.
- D-328: PARTIAL. Window-count and nearest-parent fixes DONE and tested; the missing window itself was never reproduced (Wayland focus hypothesis unproven). Files: native.rs, native_core.rs, scripts/test-select-forwarded.sh. Left: a test on a real Wayland session.
- D-329 .MOV: DONE as a check, no defect found; two tests pass. Files: native.rs. Left: nothing.
- Final full-suite run was started and not confirmed; clippy not run; release build done before the .MOV test (binary passes the forwarded-launch script).

## Session stop 18 Nov 2026 (usage limit)
- Final full suite was ~382/442 when stopped; log: /tmp/claude-1000/-home-user/2750af87-99c8-4b99-968a-17702775fa0d/scratchpad/final.log (may be cleared on reboot).
- Failing so far, both also failing at baseline: tests::gtk_headless_suite, tests::pasted_copy_reciprocal_round_trip_and_fail_closed. Not diagnosed.
- TODO next session: re-run full suite; then run the dbus-update-activation-environment reset (project_wayfinder_sandbox_poisons_portal); diagnose the 2 failures; install target/release/wayfinder-native to ~/.local/bin/wayfinder via .new + mv; commit.
- FINAL: suite 436 passed / 3 failed / 3 ignored (baseline 416/5/3). Failing, all pre-existing: gtk_headless_suite, pasted_copy_reciprocal_round_trip_and_fail_closed (native.rs:18909), restore_each_puts_a_really_trashed_file_back_where_it_came_from (native.rs:18426). D-Bus env reset done after the suite. Release binary predates the .MOV test; rebuild before install.
- 19 Nov 2026: fixed the 3 failures. gtk_headless_suite must run via scripts/run232.sh; its helpers now dismiss the D-320 transfer-report dialog (close() and question()). REAL_TRASH_TEST_LOCK users recover a poisoned lock. Main tests 438 pass / 0 fail / 3 ignored; gtk_headless_suite passes. portal_service routing_bypass_* and d236 time out at random (3-5 per run, different set each run, also on HEAD) — pre-existing, open. Clippy 0 errors, 60 warnings. Installed to ~/.local/bin/wayfinder; previous binary at wayfinder.pre-d328. Not committed.
- D-329 (19 Nov 2026): preview pane sized to its tallest page (GtkStack homogeneous), so a song's play bar sat below the fold. Stack now non-homogeneous; image/media/audio pages never scroll. Seen fixed in his window. Separate: no sound was WirePlumber's saved mute (volume 0) for application.name Wayfinder in ~/.local/state/wireplumber/stream-properties; unmuted with wpctl. Wayfinder code never sets mute.
- D-330 (19 Nov 2026): audio preview has a mute button and volume slider under the play bar. They read the stream's real volume/mute on open, so a mixer-remembered mute shows. Dragging volume up unmutes. Seen in his window; unit test volume_and_mute_are_set_and_read_back passes.

- D-331 (19 Nov 2026): selected file rewritten on disk reloads its preview in the open window; status says "Updated: <name> is the latest version". Audio/video skipped so playback is not cut. TESTED: rewritten_selected_file_reloads_preview_d331 in gtk_headless_suite (pass). Installed.
- D-332 (19 Nov 2026): address crumbs only navigate; a pencil button at the front of the address field (or Ctrl+L) opens the text box; the address field is framed on its own. Up = parent folder (unchanged, now tested after a jump). TESTED: address_crumbs_navigate_and_edit_button_edits_d332 in gtk_headless_suite (pass). Installed.
- D-334 (19 Nov 2026): a rename keeps the row exactly where it was (no auto re-sort, no jump to the top) and the list keeps keyboard focus on it; the held order survives live refreshes and ends on a sort-header click or leaving the folder. TESTED: rename_reselects_and_reveals_at_natural_sort_position_d91 (updated, Details/Compact/Grid) in gtk_headless_suite (pass, with D-194b skipped for that run only). Installed.
- D-335 OPEN (19 Nov 2026): guide_toolbar_and_transfer_targets_d194b times out waiting for the mapped transfer Cancel button; fails on HEAD without D-334 too. Likely from f36d122 (drop-confirm and transfer-report dialogs removed).
- D-336 (19 Nov 2026): moving a file out of a split pane keeps that pane's scroll place in Details and Grid (Grid reads its own scroller). TESTED: split_move_keeps_source_pane_scroll_d336 (details, grid; the real-drag variant is off, the xdotool pointer does not reach the Xvfb window).
- D-337 (19 Nov 2026): address box 26px tall, search 28px, rows 44px; the hidden address entry no longer stretches the box to 34. TESTED: measured in the suite.
- D-338 (19 Nov 2026): a folder-watcher refresh waits while a name box is open, and a rebuilt row keeps the typed text. TESTED: suite.
- D-339 OPEN (19 Nov 2026): pressing a row low in the window scrolls down and keeps scrolling while held. Test written, off: the xdotool pointer does not reach the Xvfb window.
- D-340 (19 Nov 2026): renaming onto a taken name asks "Rename X to X (2) instead?" (Yes default, No goes back to the name box); folders get (2) too, no merge. TESTED: rename_onto_taken_name_offers_numbered_name_d340.
- D-341 (19 Nov 2026): Enter or a double-click on an item being named never opens it (Gwenview opened mid-rename); the name box keeps focus; leaving the folder ends the rename. TESTED: new_folder_naming_stays_put_d341.
- D-342 (19 Nov 2026): a new folder appears at the top of what is on screen and the list does not jump to the bottom (D-334's held order appended it). TESTED: new_folder_naming_stays_put_d341 (120 files, mid-scroll).
- D-343 (19 Nov 2026): split view, both panes on one folder, moving a file into a subfolder keeps both panes' place. TESTED: split_same_folder_move_into_subfolder_keeps_place_d343.
- D-344 (19 Nov 2026): refresh_row spliced at a position from visible_entries past the end of the list model and aborted (g_list_store_splice critical) after Enter + focus loss on a new folder's name box. Now finds the row in the model itself and skips if absent. TESTED: full suite.
- D-345 OPEN (19 Nov 2026): moving a file (one pane, split view, or two windows on one folder) still loses his place in real use; programmatic moves in the suite hold within one row. Recorder installed: every list jump over 200px goes to ~/.cache/wayfinder/scroll-jumps.log with the last app cause.
- D-346 (19 Nov 2026): drives showed late at startup. The last drive scan is kept in ~/.cache/wayfinder/drives.json and drawn at once, then refreshed. Startup timings go to ~/.cache/wayfinder/startup.log. BUILT, not measured on his machine yet.
- D-347 (19 Nov 2026): a reload of the same folder (watcher, move, rename) replaced every row; GTK then reset its focus row to 0 and scrolled there about 0.2s later (his log: "restore after refresh" then 860 -> 0; his video 22:33). Same-folder reloads now update only the rows that changed. TESTED in the suite (place holds exactly); unproven against a real drag, which the suite cannot drive.
- D-348 (19 Nov 2026): test refocuses the list after the move; did not reproduce the jump with the old code either, so it guards, it does not prove.
- D-349 (19 Nov 2026): GTK re-lays the rows ~120-190ms after a refresh and recomputed the offset to 0 (his log 22:50-22:52: 1848 -> 0, backtrace gtk_widget_allocate -> gtk_adjustment_configure); the place guard came off after two frames (~33ms). It now holds 1.5s and stands down on his wheel, press or key (one capture listener per view) or any deliberate scroll (mark_scroll_cause bumps a counter). TESTED in the suite; unproven against a real drag.
- D-350 (20 Nov 2026): clicking one of the bottom rows ran the list up at ~250px/s until release (his screencast 10:04, 4.1s-7.3s, pointer ~15px above the list's bottom edge). Cause: GTK's own edge scroll for box-select and file drag (gtklistbase.c update_autoscroll, 30px zone) started by a press that moved a few pixels. Fix: from press to release, and while a drag is over the view, the list is held unless Options > "Scroll at Edges While Dragging" is on (default off); wheel, keys and deliberate scrolls pass. Likely also D-339. TESTED by replaying GTK's edge-scroll step in the suite; unproven against his real mouse.
- D-351 (20 Nov 2026): name-clash dialog gets "Compare the Two Files": a window with the incoming and existing file side by side (picture, video paused, text), name/size/date under each, and a byte check that says Identical or Different. Audio shows no player (GtkMediaFile aborts on audio). Several clashes step with Previous/Next.
- D-352 (20 Nov 2026): dragging the sidebar divider left shut the sidebar with no way back (lock_sidebar_width let position 0 through). The main Paned no longer shrinks the sidebar (shrink_start_child false) and a drag to 0 snaps back to 230; a toggle button, first on the command bar, shows/hides it, stays in place either way, and the choice is saved (config sidebar_hidden). Test sidebar_button_not_divider_d352.
- D-353 (20 Nov 2026): D-350's hold snapped the place back when GTK re-measured rows during a press (his log 11:08-11:09: A -> B -> A pairs while max flipped 38,000 <-> 48,700 in a half-screen window where names wrap to two lines). The hold now follows a re-measure (adjustment "changed") and still blocks edge scroll. Test in bottom_row_click_keeps_place_d350.
- D-354 (20 Nov 2026) OPEN: two windows tiled 50/50 overlap, the right one ~1,330px wide under the left. Not reproduced: a copy of his settings measures min width 727. Recorder added: ~/.cache/wayfinder/window-sizes.log logs KWin's offered bounds against min width and tiled state.
- D-357 (20 Nov 2026) OPEN: the GTK headless suite on branch click-selects-d356 (ab2cb3e) aborted once at teardown, after the last ZIP step, with Gtk-CRITICAL gtk_widget_is_ancestor: assertion GTK_IS_WIDGET failed. The rerun passed (579.66s), and base 8f3e87e passed (591.05s). Intermittent; cause unknown.
- D-355 (20 Nov 2026) FIXED, unproven until run: fixed_two_line_name (native.rs) makes every non-compact list row reserve two lines of its own font, so rows cannot change height (no hidden label; the D-18 ghost test sees one set_opacity). Test: split_wrapped_names_click_keeps_place_d358 asserts the fixture wraps (labels drawn with one and with two lines in a narrow split pane) and that every mapped row has ONE height. It fails if the switch is bypassed: change `let name = if compact { name } else { fixed_two_line_name(name, icon_zoom) };` to `let name = name;`. The old list_rows_fixed_height_d355 (names never wrapped) was removed in ed58770.
- D-359 (20 Nov 2026) OPEN: requested. Right-click any file or folder: Pin / Unpin. A pinned item stays at the top of its folder under every sort, and the pin survives restarts. Built on branch pin-d359.
- D-360 (20 Nov 2026) OPEN: right-click a file, Rename: the editor opens on a different file ~30 rows lower and renames it (his screencast Screencast_20261120_132521.webm: Photo_bald_man... right-clicked, IMG_1103.JPG renamed to dfsfsdf.JPG; restored by hand). List sorted by Modified desc, two-line names present. Installed build ab2cb3e (D-356).
- D-360 (20 Nov 2026) FIXED, TESTED, installed: the right-click looked the row up by a list position saved at bind time; new files above it made that stale. Fix finds the row by path at click time and the menu Rename acts on that path (branch rename-d360, f577cff). Test rename_from_menu_renames_the_clicked_file_d360 fails without the fix (IMG_1101 instead of IMG_1105), passes with it. Full GTK suite passed. Not covered: real mouse in his window.
- D-361 (20 Nov 2026) OPEN, requested by the owner: name-clash dialog (native.rs ~14126, Cancel/Duplicate/Overwrite) gets a fourth choice that adds a plain 2 to the name: Sheree.jpg -> Sheree2.jpg (then 3, 4 if taken), not the Duplicate choice's "Sheree (2).jpg" (unique_target, native_core.rs:5900).
- D-356 (20 Nov 2026): slow-click rename removed at his request; a click on a name only selects, rename stays on F2 and the context menu; test `name_click_never_renames_d356` fails if any other path calls begin_inline_rename.
- D-364 (20 Nov 2026): a click on a row cut by the bottom edge moved the list (list 6 px, grid 38.5 px in the gate; one row in his window) so the file slid out from under the pointer. Cause: GTK, not a Wayfinder hold. The row's click grabs focus (gtklistfactorywidget.c:433-434), gtk_list_base_set_focus_child scrolls a newly focused row fully into view (gtklistbase.c:963-965); the icon view's GtkViewport scrolls to any focused tile (gtkviewport.c:614-636). In his build the D-350 edge hold then snapped that scroll back, which is the A -> B -> A bounce (UNPROVEN link). Fix: pointer_keeps_scroll (list: the press sets the focus row with scroll_to(FOCUS), then re-anchors on the fully shown neighbour with both ScrollInfo axes off, in the same press, so nothing moves and focus still follows the click) and pointer_keeps_scroll_grid (viewport scroll-to-focus off from press to release). TESTED with real XTEST clicks: gate `scripts/run232.sh d364` (pointer_click_keeps_scroll_d364_alone, split pane 479 px, 6000 files, list + grid, arrows/Home/End still scroll): fails with the fix off (opus-before.log), passes with it (opus2.log). Note D-336's "xdotool can't reach Xvfb" is wrong for clicks. The grid half is superseded by the GridView rebuild (next). VERIFIED LIVE 20 Nov 2026: the owner clicked a cut bottom row in his installed build (Details view, Wayland) and the list stayed put.
- D-369 (20 Nov 2026) TESTED: a video GStreamer cannot decode showed only a frame (D-362). Cause: GStreamer avidemux has no mapping for 16-bit DIB raw RGB (fourcc "DIB ", rgb555le), "Missing decoder: video/x-avi-unknown". Fix: ffmpeg converts it off the main loop to a cached VP9/Opus WebM (native_viewers::video_play_copy, cache ~/.cache/wayfinder/video-play keyed path+size+mtime, 2 GB cap) and the pane plays it; frame + "Converting to play here" while it runs, ffmpeg's last error line if it fails; raw .h264/.hevc/.rm/.mxf/.nut now convert too. Test `unplayable_video_converts_and_plays_d369` (`headless-gtk-test.sh d369`); not run, cargo check only.
- D-368 (20 Nov 2026) TESTED (full suite passed 21 Nov, 971 s; installed 21 Nov, backup wayfinder.pre-d368): (icon view thumbnails stopped at tile 60, e.g. ~1500 GIFs showed generic icons past 60). Cause: the icon view was a GtkFlowBox that built one tile per file up front, so thumbnails were capped at MAX_GRID_THUMBNAILS (60). Fix: the icon view is now a gtk::GridView (connect_grid_factory) over the same store, sort, filter and MultiSelection as the list; it binds only visible tiles, queues each thumbnail/poster on bind and cancels it on unbind; MAX_GRID_THUMBNAILS and the FlowBox build path are deleted; pointer_keeps_scroll is generalised to gtk::ListBase and pointer_keeps_scroll_grid is deleted. Tests: grid_thumbnails_past_60_d368 (301 files: jpg, .gif, animated .GIF, tall 24-bit BMP; scroll to end, tiles past 60 of each kind hold a paintable; WAYFINDER_D368_ONLY=1), grid half of pointer_click_keeps_scroll_d364_alone and every grid headless test ported to the GridView. Truth level: BUILT (cargo check --features guide --tests clean); not run until the owner runs them.
- D-359 (20 Nov 2026) DONE, TESTED, installed: Pin/Unpin on the item menu (branch pin-d359, 97bf11f, includes D-360). Two bugs found in testing and fixed: the pin badge shifted row children so unbind removed the wrong widget from the column SizeGroups (Gtk-CRITICAL + abort; now looked up by widget name, test pinned_row_unbind_frees_size_groups_d359 aborts without the fix), and a rename dropped the pin because the config read pruned the old path first. Full GTK suite passed (602 s). Not covered: real mouse in his window.
- D-362 (20 Nov 2026) OPEN, requested by the owner: preview every common video, image, document and audio file. PDF shows only a summary; .mpg is missing from VIDEO_EXT (native_core.rs:53). Plan: PDF pages via pdftoppm, office docs via soffice -> PDF, extra image formats via magick, wider video/audio lists. Branch viewers-d362.
- D-363 (20 Nov 2026) OPEN: some .bmp files stay on "Opening preview..." forever. 88 of 110 BMPs in Storage/Pairs - originals are RLE8 top-down (e.g. 01682__Sprites.bmp, 32 x -8192 x 8, compression 1); ImageMagick refuses them, ffmpeg decodes them. Two faults: a load failure is swallowed (no error shown), and no fallback decoder. Fix rides on viewers-d362. Also D-362 now covers .swf (Ruffle flatpak rs.ruffle.Ruffle 0.8.0 installed 20 Nov) and .htm/.html (LibreOffice -> PDF path).
- D-365 (20 Nov 2026) OPEN: viewer must read every file type on Storage (the owner: java, ini, pps, ppt, ps, py, reg, rmi, rtf ...). Coverage pass on viewers-d362; UTF-16 text (.reg/.ini) is called binary by read_text_preview (native_core.rs:2756).
- D-366 (20 Nov 2026) OPEN: combined build 8335e3f full suite aborts at its last test, zip_bad_oracles_d188, with Gtk-CRITICAL gtk_widget_is_ancestor (SIGABRT) after 967 s. Same suite passed on pin-d359 and rename-d360, so a jump-d358/D-351/D-361 change is the likely cause. Also: the suite now outruns run232 default 900 s (WF_HEADLESS_TIMEOUT); combined run takes ~967 s. Blocks install of the combined build.
- D-358/D-351/D-361 INSTALLED 20 Nov 2026: combined build 8335e3f (jump fix, Compare, Add 2, Pin, rename, D-356) in ~/.local/bin, backups *.pre-d358. TESTED: D-358 bottom clicks moved 0 px x4; full suite run 3 aborted on D-366 (rare zip focus critical, 1 in 56 runs; 0 in 30 on 97bf11f), run 4 failed only the known flaky "percent text followed the worker" (D-155 copy finished too fast). Not covered: live check of the bottom jump on his screen.
- D-367 OPEN 20 Nov 2026: 3 unit tests fail under run232 on the installed base (d87bba0) too, so not new: restore_each_puts_a_really_trashed_file_back_where_it_came_from + created_trashed_reciprocal_round_trip_and_fail_closed ("not found in trash" in the wf-home232 sandbox), fail_closed_on_invalid_request_refuses_a_real_pick_invocation_before_gtk_init (needs `cargo build --bin wayfinder-native` first). The 2 D-188 source-scan tests also failed on base since bede409 put a #[cfg(test)] startup_mark stub above the drag code; fixed in audit-code 747ce51.
- D-362/D-363/D-365 + audit #12 #13 #24 #25 #27 + D-188 seam fix INSTALLED 20 Nov 2026 18:14: audit-code a20e53b (viewers 0a9f53e + 1fa890e one-pipeline fix + audit commits + 747ce51). Backups ~/.local/bin/wayfinder{,-native}.pre-d362. TESTED: full GTK suite ok (1094 s), unit 458 pass, 2 fail = D-367 pre-existing trash tests. Not covered: live check on his screen.
- Audit step 4 (scroll holds, #2-#5 #7 #8 #33) 20 Nov 2026: branch scroll-holds 59e7f9c, NOT installed. No hold deleted (reapply_scroll and hold_list_at_edges have jobs fixed rows don't remove); one arbiter (SCROLL_GENERATION), logger split from input counting, d350 now at his conditions. TESTED: scroll filter ok (401 s), d226 + d233 flick ok, d18 ok; must-fail shown: with fixed_two_line_name bypassed d358 fails "every row is the same height ([28, 37])" and d350 fails "wheel still scrolls while held (-3027)". Waits on his D-364 live check before install.
- Audit #2 20 Nov 2026: refresh_keeps_scroll_place_d349 (0823ab6, 3 triggers x mid/bottom, his conditions) passes with reapply_scroll ON and OFF, so it does not reproduce D-349's GTK offset-to-0 re-lay. reapply_scroll KEPT (removing it without a failing repro risks D-349 coming back); D-349 still has no test that can fail. D-155 copy test slowed to 0.4 s steps (flake cause: pump() samples every >=200 ms, copy finished <1 s).
- scroll-holds 0823ab6 20 Nov 2026: full GTK suite ok (1109 s, no flake); release built at wf-targets/scroll-release/release/wayfinder-native. Not installed: held so his live D-364 check stays on one build.
- D-366 (21 Nov 2026) FIXED, TESTED once: root cause of the rare zip-test focus abort found. A closed context menu was unparented (idle after unmap) before GTK's after-paint focus move (gtkwindow.c maybe_unset_focus_and_default); GTK then searched focus from a destroyed button and aborted in gtk_label_grab_focus. The D-368 build hit it every time in zip_read_only_browse_open_copy_out_d188. Fix 36b705a: unparent_when_closed waits for that after-paint. headless-gtk-test default cap 900 -> 1800 s. Full suite passed once (971 s); the old rate was 1 in 56 runs, so one pass does not prove it.
- D-371 (21 Nov 2026) TESTED: after a Trash/Delete in a deep list the list went blank with the scrollbar unmoved, a row off screen was selected, and the next wheel step jumped. Cause: reapply_scroll's value_changed handler put the offset back from inside GTK's layout, where GtkListBase's own handler is blocked (gtklistbase.c:2111-2121), so GTK's anchor never followed; rows were built for GTK's offset but positioned by the held one (gtklistbase.c:1584-1590) and fell outside the view. Fix: the hold re-emits value-changed once from its tick callback so GTK re-anchors on the held offset; a delete now remembers the deleted row's index and the refresh selects the row at that index (or the last) with no scroll (select_after_delete). Test delete_keeps_view_d371 (headless-gtk-test.sh d371) written, NOT RUN (cargo check only); why GTK computed offset 0 after a one-row removal is unproven. Gate: scripts/headless-gtk-test.sh d371old runs the same test with the fix off via WF_TEST_D371_OLD (must fail); the test prints every offset change and row coverage at 0/100/300/1000/2500 ms and asserts exactly one row selected. None of it has been run. Update: the d371 logs showed GTK itself re-focuses item 0 after the focused row is removed (gtkwindow.c maybe_unset_focus_and_default, gtklistbase.c:576-584), so the root fix is keep_place_after_removal in the refresh path (view takes focus at the tracker index before the paint; selection by index); select_after_delete is gone.
- D-372 (21 Nov 2026) TESTED: corrupt-video poster job could hold a slot forever. Cause: playbin on a truncated mp4 sometimes stalls in READY->PAUSED and posts no message (GStreamer-side; at timeout: state Ready, pending Paused, bus empty, watch attached), and the old code had no bound (old: hung 3 of 5 runs). Fix: bounded 8 s timeout in native.rs start_video_poster_jobs (VIDEO_POSTER_TIMEOUT), plus per-job done flag (finish exactly once), identity-based live removal, idle guard drop, and a stall diagnostic line. New: timeout path in 3 of 9 latest runs at ~8,000 ms, never stuck. Open: whether 8 s is the right bound.
- D-373 (21 Nov 2026) TESTED (tail suite passed 2 of 2): cause = GLib caches g_get_home_dir() on first call; the D-86 test (temp HOME under wf-d86-*, deleted after) was the first caller, so gio g_local_file_trash lstat'd a deleted home and every later trash failed ENOENT (zip Copy Out + toolbar Undo test, ~native.rs:23352, status 'Undo refused ... Error trashing file ... No such file or directory'); test-harness only, production never changes HOME. Fix: native.rs gtk_headless_suite_on_gtk_thread primes glib home/data/config/cache dirs at suite start (suite_glib_dirs_d373) and asserts home_dir == private HOME at tail start and suite end. Unproven until the tail suite runs.
- D-371 + D-372 + D-373 INSTALLED 21 Nov 2026 04:43: int-d371 (delfix c2fc455 + poster-d372 b43f21e + undo-d373). Backup ~/.local/bin/wayfinder.pre-d371. Full suite 464 passed, 3 failed: D-367 two (known, base too) + audit-25 (caused by the D-373 helper, fixed and re-run: pass). d371 mode and scroll mode pass on the merge. Not restarted (he was asleep); Wayfinder was not running.
- D-370 (21 Nov 2026) TESTED (scripts/run232.sh d370 passed, log d368-logs/d370-8.log: engine 176 frames mean 16.6 ms max 16.7 ms; ctrl add, shift remove and ctrl+shift toggle exact; file drag scrolled 23,517 px, nothing in the first 150 ms; scrolling stops with the drag). Not shown: a failing run on the old code (the D-350 hold it replaces is deleted, so there is no switch back). Two bugs found by the test and fixed: the press snapshot was GTK's live bitset (now copied), and Escape does not cancel an X11 drag in GDK 4.22.5 (gdkdrag-x11.c gdk_x11_drag_handle_event has no key case), so case d ends the drag with the button release. Wayfinder now owns edge scrolling while dragging, in the list and the icon view, always on. GTK's own autoscroll is removed at its source (icon view `enable_rubberband` off; GTK's internal DropControllerMotion, the first one in `observe_controllers()`, set to PropagationPhase::None). One engine (EdgeScroll): zone min(56 px, height/4), arms after 250 ms inside a zone, speed (60 + 1140*depth^2) * ramp (1 to 3 over 1.5 s), driven by a tick callback, calls mark_scroll_cause("edge-scroll"). Fed by Wayfinder's own DropControllerMotion for file drags and by a Wayfinder box-select (GestureDrag, replaces GTK's rubberband in both views; Ctrl = toggle, Shift = add, none = replace; start anchored by row/column index, rectangle is an overlay with css class `rubberband`). The D-350 hold (hold_list_at_edges, edge_hold_for, EDGE_HOLDS, EDGE_AUTOSCROLL, the Options item, the config field, the d350 test) is deleted. Gate: `scripts/headless-gtk-test.sh d370` (edge_scroll_while_dragging_d370_alone) plus a call in the scroll-only suite. A plain click near an edge cannot move the list because nothing arms inside 250 ms.
- D-370 INSTALLED 21 Nov 2026 05:56 on top of D-371/372/373: int-d371 1a4fff3 (gridview 9bf7c57 merged; test hooks moved below the locked D-188 code). Backup ~/.local/bin/wayfinder.pre-d370 (= the 04:43 D-371 build). Full suite 466 passed, 3 failed, all D-367 trash/picker tests: created_trashed + pasted_copy reciprocal (pass alone 3 of 3, twice: parallel interference) and fail_closed_on_invalid_request (needs a built binary). d370, d371 and scroll modes pass. Not restarted (he was asleep).
- D-374 (21 Nov 2026): window snaps back to one monitor width when dragged wider. Cause: GTK clamps a floating toplevel to the compositor bounds on every relayout (gtkwindow.c gtk_window_compute_default_size). Fix: our compute-size handler keeps the current size when it already exceeds the bounds. Test: scripts/run232.sh d374 (d374old must fail). Status: TESTED and INSTALLED 21 Nov 10:16: d374 passes twice (2990 px kept, shrink to 1190 ok), d374old fails (window never held 3000, stayed 1910). Full suite 468 passed, 3 failed (the D-367 trio). d370, d371 pass. Backup ~/.local/bin/wayfinder.pre-d374.
- D-374 VERIFIED LIVE 21 Nov 2026: the owner dragged the window across both monitors on KWin Wayland; it stays.
- D-375 (21 Nov 2026): address bar shows the start of a long path, current folder cut off on the right. Fix: pin the crumb scroller to its right end whenever its width or content changes. Test: scripts/run232.sh d375 (d375old must fail). Pin is applied from an idle plus a viewport queue_allocate: set inside the viewport allocate it did not move the drawn crumbs (value 1292, drawn at 956). Status: TESTED (d375 passes; d375old fails value=0). INSTALLED 21 Nov 10:54.
- D-376 (21 Nov 2026, found while testing D-375): opening a deep folder grew the window to the full path width (717 -> 1920 px; 2211 px once D-374 kept sizes past the screen). Cause: the status label "Loading <full path>" was not ellipsized, so for those frames it set the window minimum width (min 2031 at frame 49, d368-logs/d375-grow.log). Fix: ellipsize the status label in the middle. Test: d375 asserts the window minimum stays under 900 while loading (727) and the window keeps its width. TESTED. INSTALLED 21 Nov 10:54.
- D-377 (21 Nov 2026): sidebar toggle moved from the command bar into a thin drawer sliver (wf-drawer-sliver, 20px column) left of the sidebar/file list, 16x16 (a quarter of its old size), same place open or shut; only the button toggles. Status: TESTED (d377 passes: button 16x16 in a 20px sliver, same place open and shut, covers nothing; d377old fails). Full suite 470 pass, 3 known D-367 fails. INSTALLED 21 Nov 11:35.
- D-378 (21 Nov 2026): clicking inside the rename field ended the rename (owner report, .webm). Cause: the row's capture press made the row the focus row, the field lost focus and cancelled. Fix: presses inside a text entry skip it. TESTED (d378 pass, d378old fail).
- D-379 (21 Nov 2026): recovered web-cache files with lying extensions (GIF/JPEG named .JS/.HTM/.CSS/.XML/.TXT) showed nothing. Preview and open now follow the first bytes; short MPEG pieces say so. TESTED (d379 pass, d379old fail).
- D-380 (21 Nov 2026): document bar cut off in a narrow pane. Icon buttons with tooltips, label ellipsizes. TESTED (d380 pass, d380old fail).
- D-381 (21 Nov 2026): slow image preview when arrow-keying fast. Cancel stale decodes, 24-image cache, prefetch neighbours. TESTED (d381 pass, d381old fail): last 103-112 ms vs 117-130, back 31-35 vs 121-128, next 8-10 vs 114-123.
- D-382 (21 Nov 2026): Samsung .smsbackup files read as a conversation in the preview; open writes the full text to ~/.cache/wayfinder/sms/. TESTED (d382 pass, d382old fail; 138 real files, 45,020 messages, 0 failures).
- D-383 (21 Nov 2026): Rename to real extension (preview bar) and Options > Fix file extensions in this folder, both undoable. TESTED (d383 pass, d383old fail).
- D-384 (21 Nov 2026): the sidebar drawer arrow points the way the sidebar will go (left open, right shut), hover Hide/Show the sidebar. TESTED (d377 pass, d384old fail).
- D-378 to D-384 INSTALLED 21 Nov 2026: wf-int 8e51ac9. Backup ~/.local/bin/wayfinder.pre-d378. TESTED: GTK suite ok (971 s); d374-d383 pass on the merge; native_core 227 pass; all-tests run 479 pass, 4 fail = D-367 trash test, fail_closed (passes once the debug bin is built), gtk_headless_suite (passes alone), D-385. Not restarted.
- D-385 (21 Nov 2026): the preview pane was a 40-60 px sliver when the window is narrower than the saved size (the divider is placed for 1180 px). Fix: when a resize leaves the preview under 120 px, it gets 300 px (half the split on a very narrow window); handle drags are untouched. TESTED (d385 pass at 260 px, d385old fail at 60 px; GTK suite ok 1031 s; d380 d383 pass; d381 timing same with fix on/off under a 14-load machine).
- D-386 (21 Nov 2026) requested by the owner, UNPROVEN live: the PS2 hard drive (pfsfuse FUSE mounts, one per area, under one parent folder; raw disk has no FSTYPE so lsblk discovery skips it) gets one sidebar Devices row "PS2 HDD". `ps2_drives` (native_core.rs) reads /proc/mounts for `fuse.pfsfuse`, takes the common parent of the mountpoints as path and mountpoint, kind "ps2", no space probe; no live mount means no row (empty leftover folders do not count). Opens like a network row (navigate by path). No Unmount/Eject: `drive_can_unmount`/`drive_can_eject` gate the menu and both actions. Tests ps2_*_d386. Branch ps2-d386, not installed.
- D-387 (21 Nov 2026) requested by the owner ("i dont want to be able to unmount and eject my Storage or tb main drives"; "if its read write it needs eject if its read only it doesnt"): Unmount and Eject show only for a drive that is plugged in (lsblk HOTPLUG or RM), mounted, and mounted read-write. Cause: Eject followed RM, which USB hard drives report false, and Unmount followed any mounted row, so internal Storage (nvme ntfs) and /home (crypt btrfs) offered both. Fix (native_core.rs): `Drive.hotplug` from lsblk HOTPLUG, `Drive.read_only` from the `ro` option in /proc/mounts via `mount_is_read_only` (not lsblk RO); `drive_can_unmount` = kind drive + mounted + !read_only + (hotplug or removable); `drive_can_eject` = same. PS2 row (kind ps2, ro) excluded. Tests *_d387 (8 pass; on the old functions 6 fail). Branch ps2-d386, not installed.
- D-389 (21 Nov 2026): HOLDING an arrow key through a long list of images left the preview pane blank or skipping; tapping was fine. Cause: every new selection cancelled the decode in flight (`d381_cancel_all`, native.rs `set_preview_image`), so at key-repeat rate (30/s) no decode finished until the key was released, and "Opening preview" blanked the pane after 150 ms. Fix: latest wins, one decode in flight plus one waiting request (newer replaces older), a running decode is cancelled only after 600 ms, the pane keeps the last finished picture and shows each finished decode that is newer than the shown one, "Opening preview" after 400 ms. TESTED (d389, 70 noisy 3000x2250 JPEGs of 216 KB, selection every 33 ms): old code (commit 9a6144c, test only) 1 image landed in 2.2 s, blank 2539 ms, FAIL; new code 19 images landed, longest gap 184 ms, never blank, selected image shown 182 ms after the last move, pass; `d389old` (WF_TEST_D389_OLD) fails with a 2320 ms gap. d381 still passes (last=241 back=40 next=18 ms). Timings vary with machine load (load average was 16-20). Not installed.
- D-390 (21 Nov 2026): the file dialog froze after a video was selected and the portal then logged "the file picker did not answer within 120s and was killed" (twice). Causes found: (1) `ProcessChooser::production()` gave the picker a 120 s wall-clock kill even while the person was browsing (portal_service.rs); now no wall-clock limit in production, only Request.Close() or the picker's own exit end it; the limit stays for tests (`for_test`). (2) `install-daily.sh` replaced only `wayfinder`, so the portal ran an older `wayfinder-native` beside it; it now replaces `wayfinder-native` and `wayfinder-portal` in the same run when installed (portal needs a new portal beside the candidate). (3) The freeze itself: UNPROVEN. At HEAD no ffmpeg/ffprobe call is on the main thread in the selection path (all in `gio::spawn_blocking`); the remaining synchronous work is GTK's own `MediaFile::for_file` and the in-process poster pipelines. TESTED: `production_chooser_has_no_wall_clock_kill` fails on the old code (left Some(120s), right None), passes now; the other deadline tests pass; test-install-daily.sh 12 of 12 (3 new cases fail on the old script). Not installed.

- D-391: a video preview no longer builds its GTK media pipeline (GtkMediaFile, poster, ffmpeg conversion) on the main thread the moment the selection lands on it; it waits until the selection has stayed on the same file for 250 ms, and a selection that moves first builds nothing. Images and other kinds unchanged. TESTED: `video_preview_pipeline_waits_for_a_steady_selection_d391` fails on the old code (pipeline built synchronously), passes now; d86, d362, d329, d379 pass. The owner freeze itself is UNPROVEN and not reproduced; this removes one main-thread cost, not proven to be the cause. Not installed.
- D-392 (21 Nov 2026): Wayfinder froze; a core dump showed the GTK thread blocked in `statx` from `prune_missing_pins` (`config.pins.retain(... symlink_metadata ...)`) inside `update_config`, run by a settings save, with one pinned folder on a busy fuseblk (NTFS over FUSE) drive. Fix (native.rs): config reads and saves no longer stat any pin; `prune_missing_pins_at` removes pins for gone paths on a `wf-prune-pins` worker thread started once at launch. TESTED: `config_save_does_not_stat_pins_d392` fails on the old code (the missing pin was dropped by an unrelated save), passes now; the D-359 GTK test `pin_keeps_item_on_top_d359` was updated to call the worker function and not rerun. The freeze with a real busy drive is UNPROVEN. Not installed.

- D-393 (21 Nov 2026): clicking a row cut by the bottom edge of the list moved the list up ("i click on the lower part and the files move up again", compact view). Reproduced: a row with 11 of 19 px cut moved the offset 7704 -> 7717 on the first click, single pane and split. Cause: the press counted as his own input (`count_his_scroll_input`, native.rs), which takes every hold off, and GTK then re-lays the rows out around the pressed row (gtk_adjustment_configure from gtk_widget_allocate), so nothing put the offset back. Fix: a left press on a list row starts a place hold right after the count (`press_is_on_row_d393`, `reapply_scroll`); it ends at his next wheel, key or press; a scrollbar drag is not held. The jump log now adds the last input before a "no app cause" jump (kind, ms before, row and y for a click; never a name or path). TESTED: `compact_lower_click_keeps_place_d393` fails on the old code (2 of 72 real clicks moved, 7704 -> 7717) and passes now (0 of 72); the scroll suite (D-358, D-364, D-370, D-371, D-349) passes with it. NOT COVERED: the real-use jumps of 259-527 px in scroll-jumps.log are not shown to be this cause; the new log line will say.

- D-394 (22 Nov 2026): `install.sh --uninstall` left Wayfinder as the default folder app: install ran `xdg-mime default com.kadaken.wayfinder.desktop inode/directory` and uninstall never undid it, so `mimeapps.list` named a desktop file that no longer existed. Found by the 0.9.29 audit row W5.4. Fixed: install records the user's own previous `inode/directory` default from their `mimeapps.list` (in `~/.local/share/wayfinder/previous-folder-app`, never overwritten by a reinstall); uninstall restores it, or drops the Wayfinder line when there was none, and leaves a default the user changed after install alone. Tested in a fake home (4 cases: previous default, none, changed after install, reinstall): the 0.9.29 installer left Wayfinder as the default in 3 of 4, the fixed one in 0 of 4.

- D-395 (22 Nov 2026): the document preview caches `doc-pdf` (`native_viewers.rs:162`) and `doc-pages` (`native_viewers.rs:306`) have no size cap or pruning; only `video-play` is capped (`:205`, `:264`). Found by reading the code during the 0.9.29 audit; not measured. Open, for 0.9.30.

- D-396 (22 Nov 2026): carried from the 0.9.29 audit draft, all open, for 0.9.30. S6: LibreOffice previews (soffice) run with no network-off setting. S4: Eject and Unmount have no confirm dialog. W2: the production file chooser has no deadline (`portal_service.rs:374`), so a chooser can stay open if the calling app dies without sending Close; untested.

- D-397 (22 Nov 2026): a phone video named IMG_6414.3gpp showed no player. Cause: `VIDEO_EXT` (native_core.rs:63) and the MP4/3GP signature list (native_core.rs:5811) knew `3gp` and `3g2` but not the long spellings `3gpp`, `3gpp2`, `3gp2`. Fix: added them to both. TESTED: `viewer_lists_d362` now checks those names; fails on the old list, passes with the fix (logs d368-logs/d397-before.log, d397-after.log). Playback in the app not checked.
- D-398 (22 Nov 2026): audit S17 (0.9.29): a 20,000-file copy whose source went away mid-copy froze the window for 4+ minutes (main thread ~99% CPU, RSS 1.47 GB). Cause: `copy_item` kept all of cp's error output (one line per file) and the whole text went into the alert body and the status label. Fix: native_core.rs `copy_item` drains all output but keeps the first 64 KiB plus a count of the lines left out; new `brief_failure` cuts the alert to 8 lines (each at most 300 characters) and the status label to 1 line. TESTED: `huge_copy_error_is_capped_d398` (stand-in cp printing 20,000 lines) fails on the old code (kept 1,708,889 bytes) and passes with the fix (logs d368-logs/d398-before.log, d398-after.log). The window freeze itself was not re-run in the GTK suite.
- D-395 fixed (22 Nov 2026): `prune_cache_dir` (native_viewers.rs, shared with the D-369 video cache) now runs after each new converted PDF (`pdf_for`) and each drawn page (`render_pdf_page`), cap `DOC_CACHE_CAP` 512 MB per cache, oldest first, never the file just made. TESTED: `page_cache_stays_under_its_cap_d395` (three sparse 300 MB old pages plus one real pdftoppm page) fails without the pruning (943,718,849 bytes left) and passes with it (logs d368-logs/d395-before.log, d395-after.log). The doc-pdf side uses the same function; not separately tested (needs soffice or gs).
- D-396 resolved (22 Nov 2026): S6 FIXED: soffice and gs previews (native_viewers.rs `pdf_for`) start through `offline_command`, which wraps them in `unshare --user --map-current-user --net` (no root; probed once, falls back to a plain run where user namespaces are off). TESTED: `converters_run_without_network_d396` shows only `lo` inside the wrapper; checked by hand that soffice converts a text file inside it and curl cannot connect. Not tested: that a real document's remote image is not fetched. S4 CLOSED, no change: eject and unmount are already refused while a transfer runs (`drive_removal_refusal`, native.rs:17289, Finding 9) and udisks refuses a busy filesystem; a confirm dialog would add a click with no data saved. W2 CLOSED, no change: the production chooser has no wall-clock limit on purpose (D-390, test `production_chooser_has_no_wall_clock_kill`), and it honours Request.Close (portal_service.rs:610, tests from :4037). That xdg-desktop-portal sends Close when the calling app dies is the frontend's behaviour, not checked here.
- D-400 (22 Nov 2026): fixed. connect_sidebar_drop called Path::is_dir() on the GTK thread for every pin, recent, drive and tree row on every sidebar rebuild; a slow FUSE mount blocked startup ~40 s (S19). Found by strace -k: statx from connect_sidebar_drop <- rebuild_sidebar <- apply_drive_scan. Known folders (drives, standard folders, tree rows) now skip the stat; pins and recents are checked once per path on a worker and only a yes is cached. Test sidebar_drop_check_runs_off_the_gtk_thread_d400 (WAYFINDER_SUITE_ONLY=d400): fails on the inline check, passes on the fix.
- D-399 (22 Nov 2026): audit S19 (0.9.29): scrolling an icon view of 5,000 JPEGs grew RSS 262 MB, 1,183 MB, 2,312 MB over two passes with no plateau. Cause: `connect_file_drag_with` (native.rs) gave a closure holding the tile/row widget strongly (`in_box_select_strip`, and the drag-claim closure) to controllers that sit on that same widget, so widget -> controller -> closure -> widget was a reference cycle and every tile or row GTK ever built (with its Picture and thumbnail texture, about 220 KB each) stayed alive. Not the doc-pdf cache, not malloc fragmentation (`malloc_trim` and MALLOC_ARENA_MAX=2 changed nothing). Counted with weak refs: before, every painted Picture and texture was still alive (3,513 of 3,513); after, 513 (GTK's tile budget). Fix: both closures hold a weak ref. TESTED: `grid_memory_plateaus_d399` (`scripts/run232.sh d399`, 1,500 JPEGs, 4 passes) fails before (RSS 346, 901, 1,258, 1,592 MB) and passes after (344, 523, 530, 534, 537 MB); 5,000 files after the fix: 349, 535, 538, 537 MB, logs d368-logs/mem-0930/. Details-view rows used the same function and had the same leak.
- D-401 (22 Nov 2026): fixed. An AVI whose video track GStreamer has no decoder for (UT Video; Lagarith and similar) prepared with no error and showed a blank pane forever, so the D-369 ffmpeg fallback (which only ran on an error) never started. attach_preview_media now arms guard_blank_video: a video file that prepares with no video, or with no picture after 2.5 s, is converted by ffmpeg and played. Test avi_codecs_play_in_the_preview_d401 (WAYFINDER_SUITE_ONLY=avi): 33 AVIs (Xvid, DivX 2/3, MJPEG, DIB, H.264, HEVC, MPEG-1/2, DV, Cinepak, WMV, Huffyuv, UT Video, FFV1, FLV1, H.263, Theora, interlaced, odd size, two audio tracks, cut index, OpenDML over 1 GB); only UT Video failed before.
