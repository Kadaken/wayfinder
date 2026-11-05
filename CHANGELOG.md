# Changelog

Dates are the day the work landed. Versions follow
[semantic versioning](https://semver.org): until 1.0 the interface may still
change between releases.

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
