# Findings outside scope

- 2026-08-05: Explorer-parity gaps found in the audit but not yet built: redo (Ctrl+Y needs a redo stack), Shift+Arrow range-extend selection, clickable breadcrumb segments in the address bar, rubber-band (drag-box) selection in Details view, middle-click folder to open in a new tab, a Replace/Skip/Keep-both conflict dialog (paste currently always keeps both), and group-by in Details view.
- 2026-08-05: Cleaned 96px sidebar icons (shadows stripped) are still sitting in /tmp/sbwork — not yet installed into static/icons/ or regenerated at 18px.

- 2026-08-03: The 18px sidebar icons in `static/icons/sidebar/` (and the installed copies in `~/.local/share/wayfinder/icons/sidebar/`) still carry the same baked-in grey background fringe that the app logo had — `documents.png`, `pictures.png`, `trash.png`, `storage.png`, `home.png`, `downloads.png` are the worst. Same fix applies: rebuild the alpha from a saturation mask and extend interior colour outward.
