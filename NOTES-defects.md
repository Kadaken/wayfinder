
## D-272 — the window freezes on a cold network mount

16 November 2026, the owner: "it freezes when i ry to select the new drive".
Selecting a newly mounted rclone Google Drive folder locks the window.
Measured on `/home/user/CloudDrive-user`: the first directory read took
**7.8 seconds**, the second 0.24 s once the FUSE dir cache was warm. The read
is happening on the GTK main thread, so the whole window stops answering for
as long as the network takes instead of showing progress and staying usable.
Not a hang — it does come back. Reproduce by mounting any rclone remote and
selecting it before anything has cached.

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
0.9.25 put the old drawings back, at the right size. the owner: "they are the old
icons not the new ones."

The 256px masters are now `static/icons/sidebar-src`, recovered from commit
b5af50a, and `scripts/make-sidebar-icons.sh` shrinks from there. The 96px set
is deleted -- from the repo, from the leftover release tree in
`~/Projects/Scratch/wf-release-0dd0268`, and from
`~/.local/share/wayfinder/icons`, which an August install had left behind and
which `native.rs` already asserts the app must never read.
