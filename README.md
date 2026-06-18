# Wayfinder

A fast, native file manager for Linux — built with **Tauri 2** + **Svelte 5** + **Rust**.

Wayfinder is a lightweight, Explorer-style file manager with a **docked image preview pane**, system-themed icons, tabs, and full file operations. It was born out of frustration with existing Linux file managers that either lacked a clean always-on preview pane or felt heavy.

> Name: "wayfinding" is the practice of navigating to a destination — and a nod to **Wayland**, the display server it's built for.

## Features

- 🖼 **Docked preview pane** — select an image and see it large, always-on, no popups
- 🗂 **Tabs** — multiple folders in one window (`Ctrl+T` / `Ctrl+W`)
- 🎨 **System icons** — per-file-type icons pulled from your active icon theme (Breeze, etc.)
- 📋 **Full file ops** — cut, copy, paste, move, rename, delete-to-trash, permanent delete, new folder
- 🎯 **Drag & drop** — drag files onto folders, tabs, or sidebar places (Ctrl = copy)
- 🖱 **Two context menus** — one for sidebar places, one for files (with "Open With" app list)
- ⭐ **Favorites & Recent** — pin folders, auto-tracked history (persisted)
- 🔍 **Search**, sortable columns, **Properties** dialog
- 💾 **Drives & cloud** — auto-detects mounted drives + rclone/Google Drive mounts, mounts on click
- 🔎 **Per-pane Ctrl+scroll zoom** — zoom file icons or the preview image independently
- 📐 Details + icon views, address bar, back/forward/up navigation

## Requirements

- A Linux desktop (developed on Fedora KDE / Wayland)
- `xdg-utils`, `gio` (glib2), `udisks2` — for opening files, trash, and mounting
- A standard freedesktop icon theme (e.g. `breeze-icons`)

## Build from source

Prerequisites: [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20+, and the
[Tauri 2 Linux dependencies](https://v2.tauri.app/start/prerequisites/) (`webkit2gtk4.1`, etc.).

```bash
git clone <your-repo-url> wayfinder
cd wayfinder
npm install

# run in dev (hot-reload)
npm run tauri dev

# build a release binary
npm run tauri build -- --no-bundle
# binary -> src-tauri/target/release/wayfinder

# or build installable packages (rpm/deb)
npm run tauri build -- --bundles rpm,deb
```

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+T` / `Ctrl+W` | New / close tab |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste |
| `Ctrl+A` | Select all |
| `F2` | Rename |
| `Delete` | Move to Trash |
| `Backspace` | Up a folder |
| `Alt+←` / `Alt+→` | Back / forward |
| `Ctrl+L` | Focus address bar |
| `F5` | Refresh |
| `Ctrl+H` | Toggle hidden files |
| `Ctrl + scroll` | Zoom icons (file pane) or image (preview pane) |

## License

MIT — see [LICENSE](LICENSE).
