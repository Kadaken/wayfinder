
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

## D-274 — the headless GTK suite fails on a sidebar geometry assertion

16 November 2026. `scripts/run232.sh full` fails at `src-tauri/src/native.rs`
line 28042, `protected sidebar and row geometry moved`: the Home row measures
107 px before the first drive scan lands and 38 px after it, so the "geometry
did not move" assertion breaks.

    left  (after the scan):  (216, 38, 188, 680, 680)
    right (before the scan): (216, 107, 188, 680, 680)

Not caused by 0.9.24. Proven by building the previous release commit (1cf820a,
0.9.23) in a throwaway worktree and running the same suite: it fails there
too, with the same two tuples, at that commit's line 27526. So 0.9.23 shipped
with this suite already red.

Most likely a fixture artefact rather than an app fault: with only the places
rows present the sidebar's height is shared between few rows, so each is tall,
and each shrinks once the drive rows arrive. The assertion captures `before`
at a moment when the row count is different, not when the layout is wrong.
Needs deciding: either the assertion captures after the row set is final, or
the sidebar rows must not stretch at all.
