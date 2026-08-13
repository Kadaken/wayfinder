# Old Wayfinder (Tauri/Svelte) — carry-over record

Written 2026-08-13, before deleting the old Tauri/WebKit build.

**Why this file exists.** The old version is the only written record of what
Wayfinder is supposed to do and look like. Once `src/`, `src-tauri/src/lib.rs`
and the `wayfinder-old` binary are gone, this file replaces them as the spec.
Nothing here should be lost from the native GTK4 app.

Sources audited: `src/routes/+page.svelte` (1,638 lines, the whole UI) and
`src-tauri/src/lib.rs` (37 `#[tauri::command]` backend calls).

---

## 1. Visual reference — the palette

Taken from the old stylesheet, with how many times each colour was used. These
are already applied as `@define-color` tokens at the top of `APP_CSS` in
`src-tauri/src/native.rs`.

| Role | Colour | Uses in old app | Native token |
|---|---|---|---|
| Controls / buttons | `#2c3038` | 10 | `wf_control` |
| Raised panels | `#23262d` | 6 | `wf_raised` |
| Chrome (titlebar, tabs, status) | `#15171b` | 6 | `wf_chrome` |
| Window background | `#1b1d22` | 4 | `wf_base` |
| Control borders | `#3a3f49` | 8 | `wf_border` |
| Primary text | `#e3e5ea` | 8 | `wf_text` |
| Dim text | `#8b909a` | 8 | `wf_text_dim` |
| Muted text | `#6b7079` | 11 | `wf_text_muted` |
| Accent | `#3a6df0` | 20 | `wf_accent` |
| Accent hover | `#3b82f6` | 2 | `wf_accent_hover` |
| Danger / close | `#c0392b` | 2 | `wf_close` |

Also present in the old app and worth keeping in mind: `#353b45` (hover),
`#444a55` (raised border), `#cdd2da` / `#c4c8cf` / `#aeb3bb` (secondary text
ramp), `#f5f7fb` / `#eceef2` (light accents on selected rows).

---

## 2. The old backend — 37 commands

Every one of these was callable from the UI. Grouped by area.

- **Listing / navigation** — `list_dir`, `list_subdirs`, `list_drives`,
  `parent_dir`, `standard_dirs`, `app_dirs`, `start_path`, `start_select`
- **File operations** — `copy_paths`, `move_paths`, `delete_paths`,
  `delete_permanent`, `rename_path`, `make_dir`, `new_file`, `unique_target`,
  `resolve_paste`, `check_conflicts`
- **Trash** — `restore_from_trash`, `empty_trash`
- **Archives** — `compress_zip`, `extract_archive`
- **Drives** — `mount_drive`, `unmount_drive`, `eject_drive`, `whole_disk_of`
- **Opening** — `open_path`, `open_with`, `open_with_apps`, `open_terminal`,
  `new_window`
- **Search** — `search_dir`, `file_contains`
- **Preview / media** — `read_text_preview`, `read_data_url`, `is_image`,
  `icon_name`, `icon_svg`, `guess_video_mime`, `media_url`,
  `start_media_server`, `serve_media_conn`, `b64`
- **Picker mode** — `parse_picker_state`, `picker_options`, `picker_confirm`,
  `picker_cancel`
- **Misc** — `properties`, `percent_encode_segment`, `percent_decode`,
  `random_token`, `run`, `run_cmd`

---

## 3. Keyboard — the old app's full map

Read directly out of `onKey`. Everything here works in the native app **except
where noted**.

| Key | Action |
|---|---|
| `Backspace` | Back |
| `Alt+Left` / `Alt+Right` | Back / Forward |
| `Alt+Up` | Up one folder |
| `Delete` | Delete (to trash) |
| `F2` | Rename |
| `F5` | Reload folder |
| `Ctrl+Z` | Undo |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / Cut / Paste |
| `Ctrl+Shift+C` | Copy as path |
| `Ctrl+A` | Select all |
| `Ctrl+T` / `Ctrl+W` | New tab / Close tab |
| `Ctrl+L` | Edit address bar |
| `Enter` | Open selected |
| `Up` / `Down` | Move selection, scrolling it into view |
| any letter | Type-ahead jump, **800ms buffer timeout** |
| `Escape` | Close menu/dialog, clear selection; aborts picker mode |

Two behaviours worth preserving that are easy to miss:

- **Shortcuts are swallowed while a dialog is open**, so `Delete` can't hit the
  file list underneath a modal. `Escape` closes the dialog.
- **Shortcuts are ignored while typing in an input.**

---

## 4. Right-click menus

Item menu: `Open`, `Open in New Tab`, `Open in New Window`, `Open with…`,
`Cut`, `Copy`, `Copy as Path`, `Rename`, `Compress to ZIP`, `Extract Here`,
`Delete`, `Add to Favorites` / `Remove from Favorites`, `Properties`.

Background menu: `New Folder`, `New File`, `Paste`, `Open in Terminal`,
`Properties`.

Drive menu: `Open`, `Mount`, `Unmount`, `Eject`.

Trash: `Empty Trash`, `Restore`.

---

## 5. Settings the user could change

- `showHidden` — show hidden files
- `searchContent` — search inside file contents, not just names
- `thumbsEnabled` — thumbnails on/off
- `colSize`, `colType`, `colDate` — show/hide each column individually
- `justNow` — relative dates ("Just now") vs absolute
- View mode: Details / Icons / Compact list
- Preview pane open/closed; folder tree open/closed

---

## 6. VERIFIED GAPS — in the old app, missing from native

Confirmed by reading both codebases, not by guessing.

| # | Feature | Old | Native | Notes |
|---|---|---|---|---|
| 1 | ~~Lasso / rubber-band selection~~ | `lassoStart` / `lassoMove` / `lassoEnd` | **DONE 2026-08-13** | Enabled on the virtualized ListView via the `enable-rubberband` property. Still missing in Grid/Icons view, which is a GtkFlowBox and has no such property. |
| 2 | **Open in New Window** | `newWindow` command + menu item | **missing** | Menu entry and backend call both absent. |
| 3 | **Paste conflict dialog** | `Replace` / `Skip` / `Keep Both` buttons | **missing** | Native silently keeps both, every time. No choice offered. |
| 4 | **Show/hide individual columns** | `colSize`, `colType`, `colDate` | **missing** | No UI to turn columns off. |
| 5 | **Thumbnails on/off switch** | `thumbsEnabled` | **missing** | Thumbnails work, but can't be turned off. |
| 6 | **Picker confirm/cancel** | `picker_confirm` / `picker_cancel` | **different** | Native uses `picker_write_and_exit`; needs testing against real callers. |

### Confirmed already carried over

Drop onto a folder row, folder-tree expansion, copy-as-path, thumbnails,
"Just now" dates, silent keep-both on paste, media/audio preview controls,
Ctrl+scroll zoom, search inside files, restore from trash, auto-select newest
download, favorites, recursive search, compress/extract, empty trash, open-with,
properties, batch rename, undo, tabs, drive mount/unmount/eject, show-hidden.

---

## 7. Do not delete these

`static/icons/` — the **native** app loads its 13 sidebar icons and 2 toolbar
icons from here at runtime (`old_wayfinder_asset_icon_path`,
`wayfinder_ui_icon_path` in `native.rs`). It lives in the web tree but is not
web-only. Deleting it blanks the sidebar.
