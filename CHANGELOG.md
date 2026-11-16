# Changelog

Dates are the day the work landed. Versions follow
[semantic versioning](https://semver.org): until 1.0 the interface may still
change between releases.

## 0.9.23 — 16 November 2026

### Fixed

- **Deleting from a mounted drive said it failed when it had worked.** Trash is
  per filesystem: an item on another volume goes to that volume's own
  `.Trash-1000`, never to the home Trash. Wayfinder listed only the home Trash
  before and after the move, found no new entry, and reported the delete as
  failed — so the same folder was moved to Trash more than once by anyone who
  retried. The Trash is now resolved on the item's own filesystem, and Undo
  reads the Trash from the receipt instead of assuming the home one. Affected
  every external drive, every network mount and every FUSE mount, including
  rclone-mounted cloud storage.

## 0.9.22 — 14 November 2026

### Added

- **A front door for anyone who wants to help.** `CONTRIBUTING.md` says how to
  build it, how to run the tests, what the quickest useful contributions are
  and that forking it is what the licence is for. `SECURITY.md` says where a
  security report goes — a private advisory, never a public issue — what is in
  scope, and what has honestly not been checked yet.

### Changed

- **A shipped binary no longer names the machine it was built on.** Panic
  strings and `track_caller` paths from dependencies carried the build user's
  home folder: 151 copies of it in `wayfinder-native` and 112 in
  `wayfinder-portal`, readable with `strings` by anyone who downloaded the
  archive. Every release build now goes through `--remap-path-prefix`, and the
  packaging script refuses to build an archive whose binaries still carry a
  build path, so this cannot come back quietly.
- **Every image in this repository carries no metadata.** The five concept-art
  sheets shipped with the C2PA content-credential block their generator had
  written into them, and three icons carried creation timestamps. All of it is
  gone; the pixels were checked byte for byte before and after, and are
  identical.

### Fixed

- **A test run no longer reads the home folder of whoever runs it.** One grid
  test mirrored a real `~/Downloads` to reproduce the bug it covers. It is
  opt-in now, behind `WAYFINDER_D267_REAL_DOWNLOADS`, and reads `$HOME` rather
  than a hard-coded path.
- **A dependency could set a vector's length past its own capacity.** `rustix`
  1.1.5 marked unwritten bytes as initialized when its extended-attribute
  functions were given a zero-length buffer, which is undefined behaviour
  reachable from safe code (RUSTSEC-2026-0486). Updated to 1.1.6.
  `cargo audit` now reports no vulnerabilities at all.
- **Links pointed at a repository that had been renamed.** A crash report, the
  README, the flatpak manifest and the AppStream data all named the old
  account. The flatpak manifest also still asked for tag `v0.1.0`.
- **Test fixtures named one particular machine.** Paths and project names from
  the author's own disk were used as test data. They are neutral names now,
  which changes nothing about how the code behaves.

## 0.9.21 — 14 November 2026

### Changed

- **New sidebar icon set.** All thirteen packaged sidebar icons redrawn and cut
  at 256x256 instead of 30x30. The old ones were drawn at the slot size, which
  is why their edges looked rough. No icon carries a plate, a background or a
  coloured rim.

## 0.9.20 — 14 November 2026

### Added

- **Selecting a folder shows what is inside it.** The preview pane used to show
  two lines for a folder — the word *Folder* and its path. It now lists the
  contents: a header that opens the folder itself, then one clickable row per
  child, folders first, capped at 200 rows with a "+N more" line. A row opens
  what it names, exactly as double-clicking it in the file list would.

### Fixed

- **The folder preview is drawn from Wayfinder's own parts**, not stock GTK
  symbolic icons: Wayfinder's icon art, the file list's row padding, name font,
  hover colour and row divider.
- **A slow preview can no longer paint over a newer one.** Every preview takes
  the next generation number and a late arrival is dropped.

## 0.9.18 — 13 November 2026

### Fixed

- **Icons view lays out as a grid again.** A filename with no spaces cannot be
  broken by the default word wrap, so its label asked for its full natural
  width — 337px for one real file — and a homogeneous FlowBox handed that width
  to every column. Two tiles no longer fit a picker pane, so Icons view
  collapsed to one very wide tile per row. The picture decides the column width
  now and the name never can.
- **Tiles are picture-sized.** A square tile of 150pt at normal zoom, 90–360pt
  over the Ctrl+scroll range, with the thumbnail decoded at twice the tile and
  capped at 384px. Up to 16 columns in a wide window, was 8.

## 0.9.17 — 12 November 2026

### Fixed

- **A run of selected rows is one block of colour.** A selected row drew a pale
  outline as well as the row divider, so two selected rows together read as cut
  by a double line. The outline is gone and the divider is transparent under a
  selection.

## 0.9.16 — 11 November 2026

### Added

- **`wayfinder --select /path/to/file`** opens that file's folder with the file
  highlighted. It used to work only when the path was the very first argument
  and nothing said so, so callers passed the folder instead. The path may now
  sit anywhere among the arguments, and `--help` prints the whole command line.

## 0.9.15 — 11 November 2026

### Fixed

- **The selection survives a change of view.** Each view kept its own selection
  and switching cleared the other's without carrying it over, so anything
  selected was lost the moment the view changed — including the file Wayfinder
  had been opened at. More than one selected file is kept.

## 0.9.14 — 10 November 2026

### Fixed

- **A thumbnail falls back the way the preview does.** The preview pane learnt
  in 0.9.13 to try the outside decoder when gdk-pixbuf refuses a file; the
  thumbnail queue chose by file extension alone, so a PNG or WEBP gdk-pixbuf
  would not read kept a plain file icon for good and said nothing.
- **Every refusal is written down.** What no decoder on the machine could read
  is appended to `$XDG_STATE_HOME/wayfinder/media-failures.log` with the time,
  the stage and the decoder's own message. The log keeps its newest half once
  it passes 256 KB.

## 0.9.13 — 9 November 2026

### Fixed

- **Every picture previews, or the pane says why.** The preview pane decoded
  through one reader that handles PNG, JPEG and TIFF only, on the main thread,
  at full size, and showed an empty pane with no words when that failed. It now
  reads the header first, decodes scaled down and never up, falls back to the
  outside transcoder AVIF and HEIC already used, and when nothing on the machine
  can read the file it says so.
- **A pasted address finds its folder.** The address bar sent its text straight
  to the lister, which needs an absolute folder path, so a relative fragment, a
  `file://` URI, a quoted paste, a `~` path or a file's own path did nothing and
  said nothing. The text is cleaned and resolved — absolute, then the folder on
  screen, then its parents nearest first, then home and the sidebar's folders,
  then a bounded widest-first walk. A file settles to its folder. Nothing found
  says which text could not be found.

## 0.9.11 — 8 November 2026

### Added

- **Thumbnails in list rows.** Details and Compact draw real thumbnails for
  still images. Off by default, remembered once set, and the queue drops jobs
  for rows that have scrolled away.

### Fixed

- **Search no longer walks into what it cannot finish.** Enter in the search box
  ran a recursive walk that descended into a 4TB cloud FUSE mount and never came
  back, so searching from `/` appeared to do nothing. The walk reads the mount
  table and skips `fuse.<helper>` mounts and the kernel filesystems, with a 25
  second deadline. An ordinary NTFS disk is *not* skipped: the kernel mounts
  those as `fuseblk`, and an earlier attempt at this fix would have dropped two
  real hard drives out of every search.
- **A truncated search says so** in the status bar. Showing fewer results than
  exist, silently, reads as "the file is not on this computer".

## 0.9.7 — 7 November 2026

### Added

- **Compact view is a real view**: 16px icon, one line, no padding.
- **Home is a tree.** Desktop, Documents, Downloads, Pictures, Music and Videos
  sit under Home with a twisty; Root and Trash stay at the top level. Any
  sidebar folder opens into its subfolders at any depth, and every mounted
  device has a tree too. The arrow opens the branch, the name opens the folder.
- **Quick access and Recent files tiles** above the listing on Home. Recent is
  kept in config, newest first, no repeats.
- **AVIF, HEIC and JPEG-XL previews and thumbnails.** None had a gdk-pixbuf
  loader, so they decoded to nothing; ffmpeg decodes them once into a cached
  PNG. `heif`, `avifs` and `jxl` were not treated as images at all.
- **The installer says what it needs.** `install.sh` reports every codec and
  helper Wayfinder leans on before it copies anything, says what each missing
  one costs and gives the one command that fixes it. `--with-codecs` installs
  them, `--check` only reports. The flatpak carries ffmpeg-full, so a download
  needs nothing installed by hand.

### Fixed

- **Vault content never appears in any Recent list**, and an old config is
  scrubbed of it on every read.
- **Every sidebar folder accepts a drop**, so dragging a file onto Videos or a
  favourite works.
- **A second portal exits cleanly** instead of queueing for a name it will never
  get; 13 stale processes had piled up.
- **Recent rows drew a red broken-image square** — they asked for an icon asset
  that has never existed.
- **The sidebar is one fixed width** and cannot be dragged or collapsed by
  accident, and its names are no longer cut to twelve characters.

## 0.9.6 — 6 November 2026

### Added

- **Split view.** **F3**, or **Options -> Split View**, puts a second folder
  beside the first in the same window. The second pane is a whole pane: its own
  tabs, its own history, its own selection, sort, view mode and preview. Drag
  between the two to copy or move. F3 in either pane closes the split.

### Fixed

- **The Size column lines up with its heading.** Size was the only column whose
  values sat right while its heading sat left, so the three headings read as if
  they belonged to different columns. Heading and values are now left in every
  column.

## 0.9.5 — 6 November 2026

### Added

- **Ask on drag and drop.** Off by default, switched on in Setup or the options
  menu. With it on, a drop with no key held asks **Copy**, **Move**, **Link** or
  **Cancel** once for the whole drop, instead of the action being decided for
  you. Holding Ctrl or Shift still answers without a prompt, so the Explorer
  habit is untouched. The modifier is recorded when the drag is negotiated, not
  read again at release, so letting go of the key mid-drag cannot change the
  action that was already settled.
- **Link** as a drop action. One symbolic link per item, pointing at the
  original. Nothing is ever overwritten: a taken name is numbered "name (2)".
  All or nothing — if one link cannot be made, every link that call already made
  is removed, so a half-linked folder is never left behind. Undo removes exactly
  the links it created.
- **First-run setup.** On first launch Wayfinder asks once about the settings
  people change first: ask-on-drop, hidden files, searching inside file
  contents, and whether to empty the Trash automatically. One window with every
  option visible and what it does written next to it; Skip changes nothing.
  Reopenable any time from the options menu as **Setup...**. It never appears in
  a file dialog, because a file dialog belongs to another program.

## 0.9.4 — 6 November 2026

### Changed
- **There is always somewhere to right-click for a new folder.** A folder whose
  rows filled the window left no empty space: every pixel belonged to a file, so
  the background menu could not be reached. The list now keeps a strip of its
  own down the left side and below the last row, and a right-click there opens
  New / Paste / Properties as before.
- **Size and Type no longer run together.** Size is right-aligned, so its
  heading ended flush against the divider Type starts after and the two words
  read as one. Both the heading and the values beneath it now hold a 10px
  inset, and the column is 10px wider to pay for it.

- **The Save box is ready to type.** The suggested name was highlighted but
  nothing held the keyboard, so the first keystroke went to the file list and
  the name sat there until it was clicked. A Save picker now opens with the
  name box focused and the stem selected: type to replace it, or press Save to
  keep it. The extension is never selected, so typing cannot eat it.

### Fixed
- **Every pane divider drew two lines.** The theme painted the separator's own
  7px box on top of the 1px rule, leaving a paler hairline 3px away, so the
  boundaries either side of the file list looked ragged. One crisp line each
  now, in the border colour rather than near-black.

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
