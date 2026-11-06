# Changelog

Dates are the day the work landed. Versions follow
[semantic versioning](https://semver.org): until 1.0 the interface may still
change between releases.

## 0.9.3 — 5 November 2026

### Fixed
- **Selecting a large text file froze the window.** The preview read 256 KB
  and put all of it in one label; the toolkit lays out and measures every
  character of that before the window can draw again, so a multi-megabyte
  .txt locked the app up for seconds. The read is unchanged -- it is what
  tells text from binary -- but what is shown is now capped at 300 lines and
  16 KB, with the truncation said on screen.

## 0.9.2 — 5 November 2026

### Changed
- **The toolbar stopped looking like a toy.** Every button sat in its own
  filled, bordered box, and any toggle that was switched on was painted a
  block of saturated blue. Buttons are now invisible until the pointer is over
  them, and an active toggle is a quiet 20% accent tint with an accent glyph,
  the way a native file manager's toolbar reads. Accent is spent on one thing
  at a time instead of on every control that happens to be on.

## 0.9.1 — 5 November 2026

Two things a file dialog has to do, which it could not do before.

### Added
- **Choosing a folder works.** `OpenFile` with `directory=true` is honored
  instead of refused: the picker opens in folder mode, only folders can be
  selected, and the answer is checked to be a real directory before the app
  sees it. Apps that ask for a folder -- "choose download location", an
  importer pointed at a directory -- got a flat failure before this.
- **`SaveFiles` is exported.** An app that hands over a list of file names and
  asks where to put them now gets a folder picker and one `file://` URI per
  name, in request order, inside the folder chosen. The portal names the
  targets; it never creates them. Every name is checked to be one plain name,
  so a request can never write outside the chosen folder, duplicates are
  refused, and the list is capped at 256 names.

### Changed
- `directory=true` on a `SaveFile` is refused outright: a Save names a file.
- **A request from outside always opens a new window.** The browser's "open
  downloads folder", "show in folder" and a launcher with `%U` used to be
  routed into the window already on screen, which moved that window to
  another folder and lost the place you were in. Each such request now gets
  its own window; the window you are working in is never touched.

## 0.9.0 — 4 November 2026

First public release.

### Added
- The source is open: **GPL-3.0-or-later**, with per-file licence headers and
  `THIRD-PARTY-NOTICES.md` listing every dependency and system library.
- `packaging/install.sh`, an installer and uninstaller that works for one user
  (`~/.local`) or for everyone (`--system`), with `--prefix`, `--destdir`,
  `--no-portal` and `--dry-run`. No path is hard-coded any more: the real
  install prefix is written into the desktop entries and the D-Bus service
  files, which need absolute paths because D-Bus activation does not search
  `PATH`.
- Registration on **every desktop**, not only KDE. The portal declaration no
  longer carries `UseIn=KDE`; which backend serves the file dialog is set in
  `~/.config/xdg-desktop-portal/portals.conf`, which the installer writes.
- A **crash report**. A fault now writes the version, the time, the message,
  the place and a backtrace to `$XDG_STATE_HOME/wayfinder/`, and names that
  file on standard error, instead of the window vanishing silently.
- `--version` on both binaries.
- A flatpak manifest for the file manager, rewritten for the GNOME 50 runtime.
  It is written, not yet built: it needs a two-gigabyte SDK download, and a
  flatpak cannot be the system file dialog in any case.

### Changed
- Internal working notes moved out of the source tree.

### Known gaps
- The flatpak manifest has not been through a real `flatpak-builder` run.
- Choosing a folder (rather than a file) in the file dialog is refused, and
  `SaveFiles` is not implemented.
- Search results can go stale after files change on disk.
- No dependency CVE scan has been run, and no deliberate hostile-app run
  against the file dialog.

## 0.1.0 — 21 June 2026

Private first build, never published: core file management, the preview pane,
tabbed browsing, audio playback and rubber-band multi-select.
