# Native Linux Port

Wayfinder is already a Svelte + Tauri app, but Tauri on Linux still runs the UI in WebKitGTK. The native target is the Linux-bound replacement path: Rust + GTK4/libadwaita, no Electron and no webview for the primary file manager surface.

## Current State

- Existing Tauri shell remains available as `pnpm tauri dev` and `pnpm tauri build`.
- Immediate regression fix: Tauri native drag/drop events are re-enabled with `dragDropEnabled: true`.
- New native binary: `wayfinder-native`, built from `src-tauri/src/native.rs`.
- Shared native core: `src-tauri/src/native_core.rs` has directory listing, recursive name search, standard places, drive discovery/mount/unmount/eject, open, Open With app discovery/launch, properties, text preview, create, rename, batch rename, Trash/restore/empty, permanent delete, terminal launch, archive compress/extract, keep-both conflict naming, copy/move helpers, and move-back helpers for undo.
- Native shell now has GTK multi-select, Explorer-style dark chrome, native tabs, old-style toolbar, breadcrumb path text, details columns, Favorites/Places/Devices/Folders/Recent sidebar sections, active sidebar rows, blue drawn folder icons, folder filtering/search, Enter-to-search-subfolders, image preview, right-click context menus, selection-aware drag-out, drop-in copy, copy/cut/paste, new folder, new file, rename, batch rename, Compress to ZIP, Extract Here, Trash, Empty Trash, Undo, Shift+Delete permanent delete confirmation, copy paths, properties, Open With, terminal here, and common keyboard shortcuts.
- The installed user launcher target `/home/zito/.local/bin/wayfinder` is the native GTK4/libadwaita binary. The old Tauri binary backup is `/home/zito/.local/bin/wayfinder.pre-native-20260702-125518`.
- The native app handles desktop `%U` launches through `gio::ApplicationFlags::HANDLES_OPEN`, including plain paths and `file://` URIs.
- Directory loading is asynchronous and cancellable: late results from a slow mount cannot replace a newer folder, entries with unavailable metadata remain visible with an explanation, local directory changes trigger a debounced refresh, and only the active list/grid view is constructed on refresh. Grid thumbnails use a cancellable four-at-a-time background queue (first 60 images) rather than decoding full images on the UI thread.

## Build

Fedora dependencies:

```bash
sudo dnf install -y gtk4-devel libadwaita-devel gdk-pixbuf2-devel
```

Build the native shell:

```bash
cargo build --manifest-path src-tauri/Cargo.toml --bin wayfinder-native --features native
```

Build an optimized release:

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml --bin wayfinder-native --features native
```

Run it:

```bash
src-tauri/target/release/wayfinder-native
```

## Native Port Contract

The native app must keep file operations off the GTK main loop. Long copies, moves, archive work, and device operations should run in child processes or worker threads and report completion back to the UI.

### Reliability invariants

- Directory reads and per-entry metadata lookups run on a worker thread. Each navigation has a cancellation flag and generation number, so a slow mount can never overwrite the next folder the user opens.
- A metadata failure is represented in the listing; it must not silently remove an entry. Entries without metadata display an explicit unavailable state.
- Keep a `gio::FileMonitor` only for the current local directory and debounce its events before refreshing. Network/FUSE mounts may not emit monitor events, so the user-facing fallback remains F5.
- Construct only the active Details/Compact or Grid widgets. Do not build a hidden view during refresh.
- Thumbnail decoding is asynchronous and bounded to four jobs. Cancel the queue on any navigation, refresh, filter change, or view change; never decode full-resolution images synchronously in GTK callbacks.

The relevant implementation lives in `src/native.rs`; filesystem and operation helpers live in `src/native_core.rs`.

Drag/drop should use GTK/GDK primitives:

- Drag out: `gtk::DragSource` + `gdk::FileList` + `gdk::ContentProvider`.
- Drop in: `gtk::DropTarget` accepting `gdk::FileList`.
- Copy/move decisions should map to GDK drag actions, then file operations should run asynchronously.

## Remaining Port Work

- Port favorites/recents persistence and modern libadwaita dialog widgets.
- Consider GTK model/view virtualization before supporting folders with many thousands of visible entries.
- Add richer video/audio previews without localhost media-server workarounds. Audio intentionally opens in the user's default player instead of creating an unstable in-app GStreamer pipeline.
- Port tree sidebar, content search, and picker mode.
- Broaden real drag/drop testing across Dolphin, KDE desktop, browsers, terminals, and chat/file upload targets.
- Add a native desktop file and installer path once the GTK shell reaches parity.
