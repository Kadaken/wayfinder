# Wayfinder

A fast, native file manager for Linux — built with **Tauri 2** + **Svelte 5** + **Rust**.

Wayfinder is a lightweight, Explorer-style file manager with a docked preview pane, Windows-style relative timestamps, image thumbnails, tabs, and full file operations — built out of frustration with existing Linux file managers that felt heavy or lacked a clean always-on preview.

> "Wayfinding" is the practice of navigating to a destination — and a nod to **Wayland**, the display server it targets.

## Download

Grab the latest binary from the [**Releases**](https://github.com/Kadaken/wayfinder/releases) page — no installation required, just run it.

```bash
chmod +x wayfinder
./wayfinder
```

## Features

- **Docked preview pane** — select any file; images render large, text files show their content
- **Windows-style timestamps** — "Just now", "5 min ago", "Yesterday, 2:30 PM", or full date — auto-updating every minute
- **Image thumbnails** — grid view loads real previews for image files (toggle in ⚙ Options)
- **Tabs** — multiple folders in one window (`Ctrl+T` / `Ctrl+W`)
- **System icons** — per-file-type icons from your active icon theme (Breeze, Papirus, etc.)
- **Full file ops** — cut, copy, paste, move, rename, trash, permanent delete, new folder/file, ZIP compress/extract
- **Drag & drop** — onto folders, tabs, or sidebar places (`Ctrl` = copy)
- **USB hotplug** — drives appear automatically when plugged in; right-click to unmount or eject
- **Open With** — per-app submenu for any file type, flyout on hover
- **Favorites & Recent** — pin folders, auto-tracked recents (persisted across sessions)
- **Search**, sortable columns, Properties dialog
- **Resizable panes** — drag the splitter handles; layout saved per session
- **Ctrl+scroll zoom** — zoom icons or the preview image independently
- **Options (⚙)** — configure "Just now" duration, column visibility, thumbnail toggle

## Requirements

- Linux with a desktop session (Wayland or X11)
- `xdg-utils`, `gio` (glib2), `udisks2` — for opening files, trash, and mounting drives
- A freedesktop icon theme (e.g. `breeze-icons`, `papirus-icon-theme`)

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+T` / `Ctrl+W` | New / close tab |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste |
| `Ctrl+A` | Select all |
| `F2` | Rename |
| `Delete` | Move to Trash |
| `Backspace` | Back in history |
| `Alt+Up` | Go up to parent folder |
| `Alt+←` / `Alt+→` | Back / forward |
| `Ctrl+L` | Focus address bar |
| `F5` | Refresh |
| `Ctrl+scroll` | Zoom icons or preview image |
| `Escape` | Deselect / close menus |

## Build from source

Prerequisites: [Rust](https://rustup.rs), Node.js 20+, and the
[Tauri 2 Linux dependencies](https://v2.tauri.app/start/prerequisites/) (`webkit2gtk4.1`, etc.).

```bash
git clone https://github.com/Kadaken/wayfinder.git
cd wayfinder
npm install

# dev mode with hot-reload
npm run tauri dev

# release binary (no installer)
npm run tauri build -- --no-bundle
# binary → src-tauri/target/release/wayfinder

# installable packages (rpm / deb)
npm run tauri build -- --bundles rpm,deb
```

## License

© 2026 Kadaken — All Rights Reserved. Personal use only. Modification and redistribution are prohibited. See [LICENSE](LICENSE) for full terms.
