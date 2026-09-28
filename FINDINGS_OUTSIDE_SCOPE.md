# Findings outside scope

- STILL OPEN 2026-08-13: redo (Ctrl+Y needs a redo stack); group-by in Details view; Open in New Window (the app keeps its UI in a thread-local `CURRENT_UI`, so a genuine second window needs that unwound first); per-column show/hide and a thumbnails on/off switch in Options; Grid/Icons view is still un-virtualized (GtkFlowBox) so huge folders are slow there and it cannot do rubber-band selection.
- DONE 2026-08-13: Shift+Arrow range-extend, clickable breadcrumb segments, middle-click folder -> new tab, "Open in New Tab" menu item, and the Replace/Skip/Keep Both paste conflict dialog.

- DONE 2026-08-13: sidebar icon shadows stripped at 96px and re-rendered at 18px; static/icons/, static/icons/sidebar/ and the installed copies all updated.

- STILL OPEN 2026-09-27: `native_core::tests::guide_names_registry_is_exact` fails pre-existing (unrelated to D-153 phase 3): `guide-names.json` is missing `delete-speed` and `rename-in-place`, or the registry-derived expectation list needs updating — confirmed via `git stash` that it fails identically before this slice's changes.
