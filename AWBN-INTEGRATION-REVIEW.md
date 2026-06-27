# Wayfinder ⇄ Agent Workbench Next — file-picker integration (for review)

**From:** Agent Workbench Next (AWBN) — 2026-06-25
**Goal:** Make Wayfinder the file picker AWBN uses for its **attachment button**, and ultimately the *supreme* file picker on this Fedora box (replacing the GTK/KDE/portal chooser everywhere).

**Why this is needed:** AWBN's attachment button currently calls the OS file-**picker** portal (Tauri `@tauri-apps/plugin-dialog` `open()`), which the desktop environment provides — *not* a file manager. Wayfinder is a file **manager**; to become the picker it needs a mode that **lets the user select file(s) and returns the chosen paths to the calling app**. Wayfinder has no such mode today (the installed binary just launches the GUI; no CLI/pick flags).

---

## Tier 1 — "pick mode" CLI (unblocks AWBN now; small, self-contained)

Add a headless-invocable pick mode to the Wayfinder binary.

### Contract
```
wayfinder --pick --out <FILE> [--multiple] [--directory] [--starting-dir <DIR>] [--filter images|all]
wayfinder --pick-capable        # exit 0 if pick mode exists; for capability detection (must NOT open a window)
```

**Behaviour**
- `--pick` opens a Wayfinder window in **picker mode**: a normal browse view plus a clear **Select / Cancel** affordance (and double-click-to-select).
- `--multiple` → allow selecting more than one entry; default is single.
- `--directory` → select folders instead of files.
- `--starting-dir <DIR>` → open at this folder (default: `$HOME`).
- `--filter images` → soft-filter the view to image types (still allow browsing); default `all`.
- On **confirm**: write the chosen **absolute paths**, one per line, UTF-8, to `<FILE>`, then **exit 0**.
- On **cancel / window close with no selection**: write nothing (or truncate `<FILE>` to empty) and **exit non-zero**.
- `--pick-capable` prints nothing, opens no window, and exits `0` so callers can detect support cheaply.

**Notes**
- Absolute paths only (no `~`, no URIs). One path per line. No trailing commentary.
- Keep it a single-shot process: launch → user picks → write → exit. No daemon needed for Tier 1.
- This is a Tauri app, so pick mode = parse `std::env::args()` in `main`/`setup`, set a window/route flag (e.g. `?mode=pick`), and on confirm write the out-file via Rust before `app.exit(0)`.

### AWBN side (we implement once `--pick-capable` ships)
`chooseAttachments()` will: if a `wayfinder` binary exists **and** `wayfinder --pick-capable` exits 0 → spawn `wayfinder --pick --multiple --out <tmp>`, await exit, read newline paths from `<tmp>`, attach them. **Else fall back** to the current native picker. So attachments keep working until Wayfinder is ready, then transparently switch. (We will not flip the picker before `--pick-capable` exists — that would break attachments.)

### Acceptance (Tier 1)
- [ ] `wayfinder --pick-capable` exits 0, opens no window.
- [ ] `wayfinder --pick --out /tmp/sel.txt` → pick one file → `/tmp/sel.txt` contains its absolute path, exit 0.
- [ ] `--multiple` returns N lines for N selections.
- [ ] Cancel → exit non-zero, no stale paths written.
- [ ] `--directory` and `--starting-dir`/`--filter` honored.

---

## Tier 2 — be the *system* picker everywhere (the "replace anything else" goal)

To make Wayfinder the picker for **all** apps (not just AWBN), implement the **XDG Desktop Portal `org.freedesktop.portal.FileChooser`** backend and register Wayfinder as the portal's file-chooser implementation. Then every portal-using app (Tauri, GTK, Flatpak, browsers) opens Wayfinder to pick. This is the real "supreme file manager" path; it's a bigger task than Tier 1 and can follow it.

Also (low-risk, separate): set Wayfinder as the default **file manager** for opening folders:
`xdg-mime default <wayfinder>.desktop inode/directory` (so "open folder" actions use Wayfinder). Reversible.

---

## Suggested order
1. Tier 1 `--pick` + `--pick-capable` (unblocks AWBN attachments). ← start here
2. AWBN wires the Wayfinder picker with fallback (AWBN side).
3. Tier 2 portal backend for system-wide adoption.

Ping AWBN when `--pick-capable` lands and we'll wire step 2 the same day.
