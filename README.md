# Wayfinder

> Current native app (2026-08-13): Wayfinder is a Rust + GTK 4/libadwaita desktop application, and nothing else. The Tauri/Svelte/WebKit shell was **removed** on 2026-08-13 — there is no JavaScript toolchain, no `package.json`, and no library target. All work belongs in `src-tauri/src/native.rs` and `native_core.rs`. What the old shell did is recorded in `OLD-VERSION-CARRYOVER.md`.

**A fast, native file manager for Linux** — built for GTK 4/libadwaita, Wayland, and X11 without Electron or a webview in the primary app.

Wayfinder is a dark, keyboard-driven file manager with a docked preview pane, Windows-style timestamps, image thumbnails, USB hotplug, tabs, and full file operations.

---

## Screenshot

![Wayfinder](screenshots/screenshot-details.png)

---

## Install

Grab a packaged build from the [**Releases**](https://github.com/LegendofZito/wayfinder/releases) page when one is available, then make it executable:

```bash
chmod +x wayfinder-linux-x86_64
./wayfinder-linux-x86_64
```

Or install the RPM on Fedora/RHEL:

```bash
sudo rpm -i Wayfinder-0.1.0-1.x86_64.rpm
```

For a local native build, see [Build from Source](#build-from-source).

---

## Requirements

- Linux (Wayland or X11)
- `xdg-utils` — for opening files with default apps
- `gio` / `glib2` — for Trash support
- `udisks2` — for mounting and ejecting drives
- A freedesktop icon theme (`breeze-icons`, `papirus-icon-theme`, etc.)
- GTK 4 and libadwaita runtime libraries — required by the native app

---

## Features

### Navigation

| Feature | Details |
|---------|---------|
| **Breadcrumb path bar** | Shows `Computer › home › folder` — click any segment to jump there |
| **Click to edit path** | Click the empty space to the right of the breadcrumbs to type a path directly |
| **Paste a path** | Right-click the path bar — if your clipboard has an absolute path (`/…`), navigates there instantly |
| **Back / Forward** | Full browser-style history per tab. `Backspace` = back, `Alt+←/→` = back/forward |
| **Up to parent** | `Alt+Up` or the ↑ toolbar button |
| **Home button** | ⌂ in the toolbar — jumps to your Home folder |
| **Desktop integration** | Opening a folder from the desktop shows that folder's contents in Wayfinder; opening a file shows and selects it in its parent folder |
| **Address bar edit** | `Ctrl+L` or double-click the path bar — type any path, press Enter |
| **Refresh** | `F5` or the ⟳ toolbar button |
| **Type-ahead** | Start typing a filename — selection jumps to the first match |
| **Arrow key nav** | `↑`/`↓` to move selection, `Enter` to open |

---

### Tabs

| Feature | Details |
|---------|---------|
| **Multiple tabs** | `Ctrl+T` opens a new tab at the current folder |
| **Close tab** | `Ctrl+W` or click the × on the tab |
| **Switch tabs** | Click any tab |
| **Drop onto tab** | Drag files and drop onto a tab to move/copy them there |
| **Per-tab history** | Each tab keeps its own back/forward history independently |

---

### File Views

| Feature | Details |
|---------|---------|
| **Details view** | Table with Name, Size, Type, Modified — click column headers to sort |
| **Icons / Grid view** | Large icon grid — zoom with `Ctrl+scroll` |
| **Image thumbnails** | In grid view, the first 60 images load in the background (two at a time). A file-type icon remains visible until each thumbnail is ready. |
| **Video pictures** | In grid view, each video shows a real frame pulled from the file (two at a time). The generic video icon remains visible until a frame is ready. |
| **Sort** | Click any column header to sort ascending; click again for descending |
| **Folders before files** | Folders stay above files regardless of sort direction. |
| **Choose columns** | Right-click the Details or Compact header to show or hide Size, Type and Modified. Name always stays. The choice is remembered. |
| **Search / Filter** | Type in the search box to live-filter the current folder |
| **Hidden files** | Toggle with the checkbox at the bottom of the sidebar |
| **Icon zoom** | `Ctrl+scroll` in the file area to scale icons from 60% to 240% |

### Performance and reliability

Wayfinder keeps directory work off the GTK interface thread. Opening a slow directory, including an rclone/FUSE mount, should leave the window usable while the listing loads. If a newer folder is opened before an older load finishes, the older result is ignored.

- A file that cannot provide size or modified-time metadata remains visible and is marked as unavailable instead of disappearing.
- Local folders refresh automatically after file changes. For cloud and network mounts that do not support Linux change notifications, use **F5** or the refresh button.
- Only the active Details or Grid presentation is built. Grid thumbnails are cancellable when you change folder or view.

---

### Preview Pane

| Feature | Details |
|---------|---------|
| **Always-on preview** | Right pane shows the selected file — no popups needed |
| **Image preview** | Renders images up to 60 MB with zoom support |
| **Preview zoom** | `Ctrl+scroll` inside the preview pane to zoom in/out |
| **Video preview** | Plays compatible video in the preview pane using GTK media controls |
| **Audio files** | Show file information in the preview pane; open them in the system's default music player for playback |
| **Text preview** | Text, code, Markdown, logs, JSON — shows content up to 256 KB |
| **Binary detection** | Files with binary content show a placeholder instead of garbled text |
| **File metadata** | Below the preview: name, type, size, modified date |
| **Toggle preview** | ▭ toolbar button or drag the splitter handle to resize |
| **Resizable pane** | Drag the splitter between the file list and preview |

Media opened from Wayfinder uses your desktop's default application. Playback volume is controlled by that player and its PipeWire/Plasma per-application volume, not by Wayfinder's file list.

---

### File Operations

| Operation | How |
|-----------|-----|
| **Open** | Double-click or `Enter` |
| **Open With** | Right-click → Open With → hover to see app list flyout |
| **Copy** | `Ctrl+C` or right-click → Copy |
| **Cut** | `Ctrl+X` or right-click → Cut |
| **Paste** | `Ctrl+V` or right-click → Paste (in destination folder) |
| **Rename** | `F2` or right-click → Rename |
| **Delete to Trash** | `Delete` key or right-click → Move to Trash |
| **Permanent delete** | Right-click → Delete permanently (with confirmation dialog) |
| **New folder** | Right-click empty area → New Folder |
| **New file** | Right-click empty area → New File |
| **Empty Trash** | Navigate to Trash → right-click → Empty Trash |
| **Select all** | `Ctrl+A` |
| **Multi-select** | `Ctrl+click` to toggle, `Shift+click` to range-select |
| **Compress to ZIP** | Right-click selection → Compress to ZIP |
| **Extract archive** | Right-click a ZIP/TAR/TGZ → Extract Here |
| **Properties** | Right-click → Properties — shows size, permissions, dates, item count |
| **Open Terminal Here** | Right-click → Open Terminal Here |

---

### Drag & Drop

| Feature | Details |
|---------|---------|
| **Move** | Drag files onto a folder, sidebar place, or tab |
| **Copy** | Hold `Ctrl` while dropping — copies instead of moves |
| **Drop targets** | Works on folders in the file list, sidebar places, favorites, and tabs |
| **Visual feedback** | Drop targets highlight in blue while dragging over them |

---

### Sidebar

| Section | Details |
|---------|---------|
| **Favorites** | Pin any folder — right-click a place → Add to Favorites; right-click a favorite → Remove |
| **Places** | Home, Desktop, Documents, Downloads, Pictures, Music, Videos, Trash, Root — auto-detected at launch |
| **Devices** | All mounted drives and rclone/cloud mounts. Click to navigate; if unmounted, mounts on click |
| **Folders (Tree)** | Collapsible folder tree rooted at Home — click a folder row to expand and navigate |
| **Recent** | Last 12 folders visited, auto-tracked per session and saved across restarts |
| **Hidden files toggle** | Checkbox at the bottom of the sidebar |
| **Sidebar** | Favorites, places, devices, a Home-folder tree, and recents are available in one pane |

---

### Drives & USB

| Feature | Details |
|---------|---------|
| **USB hotplug** | Drives appear in the sidebar automatically when plugged in — no manual refresh |
| **Mount on click** | Unmounted drives are mounted when you click them |
| **Unmount** | Right-click a mounted drive → Unmount |
| **Eject (safe remove)** | Right-click a USB/removable drive → Eject — safe to remove |
| **Cloud / network mounts** | rclone mounts (Google Drive etc.) appear alongside local drives |

### Google Drive and other cloud mounts

Wayfinder treats an rclone-mounted Google Drive as a folder. You can browse, open, copy, move, rename, and delete items to the extent rclone and the account permissions allow.

- Sharing, permission changes, and creating Drive links are Google Drive web/app features; use Google Drive in a browser for those tasks.
- A Google server download-quota or permission error cannot be fixed by changing file managers. Wayfinder will keep the file entry visible where possible; retry after the account/link issue is resolved.
- Cloud mounts may cache directory data and may not emit change notifications. Refresh with **F5** after an upload, download, or remote change.

---

### Timestamps

Timestamps update live every minute — no refresh needed.

| Age | Display |
|-----|---------|
| < 2 minutes | **Just now** |
| 2–59 minutes | **5 min ago** |
| 1–12 hours | **3 hr ago** |
| Earlier today | **Today, 2:30 PM** |
| Yesterday | **Yesterday, 11:15 AM** |
| This week | **Mon, 9:45 AM** |
| This year | **Jun 3, 4:00 PM** |
| Older | **Jun 3, 2023, 4:00 PM** |

All times use a 12-hour clock tied to your system timezone.

---

### ⚙ Quick actions

The toolbar’s **⚙** menu provides the actions used most often while browsing:

| Action | What it does |
|--------|--------------|
| **New Folder / New File** | Creates an item in the current folder |
| **Refresh** | Reloads the current folder, useful for cloud/network mounts |
| **Show Hidden Files** | Toggles files whose names begin with `.` |
| **Rename / Copy / Cut / Paste / Move to Trash** | Performs the matching operation on the selection |
| **Undo** | Reverses the most recent supported operation |
| **Open Terminal Here** | Opens a terminal in the current folder |

The view mode, sort order, hidden-files preference, favorites, recents, and search-in-files preference persist across restarts.

---

### Keyboard Shortcuts — Full Reference

#### Navigation
| Key | Action |
|-----|--------|
| `Backspace` | Go back in history |
| `Alt+←` | Go back |
| `Alt+→` | Go forward |
| `Alt+Up` | Go up to parent folder |
| `Ctrl+L` | Focus address bar (type any path) |
| `F5` | Refresh current folder |
| `Enter` | Open selected file or folder |
| `↑` / `↓` | Move selection up / down |
| `Escape` | Deselect all / close menus and dialogs |

#### Tabs
| Key | Action |
|-----|--------|
| `Ctrl+T` | New tab (opens at current folder) |
| `Ctrl+W` | Close current tab |

#### File Operations
| Key | Action |
|-----|--------|
| `Ctrl+C` | Copy selection |
| `Ctrl+X` | Cut selection |
| `Ctrl+V` | Paste |
| `Ctrl+A` | Select all |
| `F2` | Rename selected file |
| `Delete` | Move to Trash |

#### View
| Key | Action |
|-----|--------|
| `Ctrl+scroll` (file pane) | Zoom file icons in/out |
| `Ctrl+scroll` (preview pane) | Zoom preview image in/out |

#### Type-ahead
Start typing any letter — the selection jumps to the first file whose name starts with what you typed. Clears after 800 ms of no input.

---

### File Type Icons

Wayfinder uses your system's freedesktop icon theme to show per-type icons. Recognized types include:

- **Audio**: mp3, wav, flac, ogg, m4a, aac, opus, wma, aiff, alac
- **Video**: mp4, mkv, webm, mov, avi, wmv, m4v, mpg, mpeg, 3gp, flv
- **Image**: png, jpg, gif, bmp, webp, tiff, svg, ico, avif, heic
- **Documents**: pdf, doc/docx, odt, rtf, xls/xlsx, ods, ppt/pptx, odp
- **Archives**: zip, tar, gz, tgz, bz2, xz, 7z, rar, zst, cab
- **Code**: py, js, ts, tsx, jsx, rs, c, cpp, h, java, go, sh, bash, html, css, json, xml
- **Fonts**: ttf, otf, woff, woff2
- **Disk images**: iso, img, dmg
- **Packages**: rpm, deb, AppImage

---

### Persistence

The native configuration currently saves:

| What | Saved |
|------|-------|
| Favorites | Yes — survives restarts |
| Recent folders | Yes — last 12, survives restarts |
| Hidden-files preference | Yes |
| Chosen view and sort order | Yes |
| Search-in-files preference | Yes |

---

## Build from Source

The native GTK application is the primary Linux build.

On Fedora, install the development libraries once:

```bash
sudo dnf install gtk4-devel libadwaita-devel gdk-pixbuf2-devel
```

```bash
git clone https://github.com/LegendofZito/wayfinder.git
cd wayfinder

# Test suite
cargo test --manifest-path src-tauri/Cargo.toml

# Optimized binary
cargo build --release --manifest-path src-tauri/Cargo.toml
# → src-tauri/target/release/wayfinder-native

# Optional user-local install (the only sanctioned way to replace the daily app)
scripts/install-daily.sh <change-label> src-tauri/target/release/wayfinder-native
```

**Hard rule: no daily replacement without a verified rollback copy first.**
`scripts/install-daily.sh` saves the current `~/.local/bin/wayfinder` as
`wayfinder.pre-<change-label>-<old-short-sha>`, verifies it, and only then
swaps in the new binary atomically. Never copy over the daily by hand. Its
fixture tests are `scripts/test-install-daily.sh`.

The `--features native` flag is gone: GTK is no longer optional, because there
is no other shell to build. The old `npm run native:build` script is gone too —
use the `cargo` commands above.


---

## License

© 2026 LegendofZito — All Rights Reserved.

Personal use is permitted. Modification, redistribution, and commercial use are prohibited without written permission. See [LICENSE](LICENSE) for full terms.
