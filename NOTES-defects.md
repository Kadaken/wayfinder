
## D-272 — the window freezes on a cold network mount

16 November 2026, Zito: "it freezes when i ry to select the new drive".
Selecting a newly mounted rclone Google Drive folder locks the window.
Measured on `/home/zito/GoogleDrive-cgzito`: the first directory read took
**7.8 seconds**, the second 0.24 s once the FUSE dir cache was warm. The read
is happening on the GTK main thread, so the whole window stops answering for
as long as the network takes instead of showing progress and staying usable.
Not a hang — it does come back. Reproduce by mounting any rclone remote and
selecting it before anything has cached.
