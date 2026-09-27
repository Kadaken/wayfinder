mod native_core;

use adw::gtk;
use adw::gtk::gdk::prelude::ToplevelExt;
use adw::gtk::{gdk, gio, glib};
use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use native_core::{Drive, Entry, ResolvedItem};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The drag picture is a ghost of the dragged row at about half opacity.
const DRAG_GHOST_OPACITY: f64 = 0.55;
/// The ghost is one item's icon and name, never wider than this many pixels.
const DRAG_GHOST_MAX_WIDTH: i32 = 240;
const DRAG_GHOST_ICON: i32 = 20;
const MAX_GRID_THUMBNAILS: usize = 60;
const MAX_CONCURRENT_THUMBNAILS: usize = 4;

const APP_CSS: &str = r#"
/* ONE palette. Every surface, line and accent below refers to these names.
   Before this existed the stylesheet had 11 surface greys (including #2a2f38
   AND #2a2f39), 4 near-identical near-black borders, 19 text colours and 3
   different accent blues — and the titlebar was neutral #303030 while the rest
   of the app was blue-tinted, so it read as bolted on. Add colours HERE, not
   inline, or the drift comes straight back.

   These values are taken from the ORIGINAL Tauri/Svelte Wayfinder
   (`src/routes/+page.svelte`), which is the visual reference this native shell
   has to match. Counts from that file: #2c3038 controls (10 uses), #23262d
   raised (6), #15171b chrome (6), #1b1d22 base (4), #3a3f49 control border (8),
   #e3e5ea text (8), #8b909a dim (8), #6b7079 muted (11), #3a6df0 accent (20). */
@define-color wf_chrome        #15171b;
@define-color wf_base          #1b1d22;
@define-color wf_raised        #23262d;
@define-color wf_control       #2c3038;
@define-color wf_control_hover #353b45;
@define-color wf_line          #0f1013;
@define-color wf_border        #3a3f49;
@define-color wf_accent        #3a6df0;
@define-color wf_accent_hover  #3b82f6;
@define-color wf_close         #c0392b;
@define-color wf_text          #e3e5ea;
@define-color wf_text_dim      #8b909a;
@define-color wf_text_muted    #6b7079;

window.wayfinder-window {
    background: @wf_base;
    color: @wf_text;
}

.wf-root {
    background: @wf_base;
    color: @wf_text;
    font-size: 13px;
}

button:focus,
button:focus-visible,
row:focus,
row:focus-visible {
    outline: none;
    box-shadow: none;
}

.wf-titlebar {
    background: @wf_chrome;
    color: @wf_text_dim;
    border-bottom: 1px solid @wf_line;
    min-height: 30px;
}

.wf-titlebar-center {
    min-height: 30px;
}

.wf-window-title {
    color: @wf_text_dim;
    font-size: 13px;
    font-weight: 700;
}

.wf-title-button {
    background: transparent;
    color: @wf_text_dim;
    border: 0;
    border-radius: 0;
    min-width: 32px;
    min-height: 30px;
    padding: 0;
    margin: 0;
}

.wf-title-button:hover {
    background: @wf_control_hover;
    color: #ffffff;
}

.wf-title-button.wf-close-button:hover {
    background: @wf_close;
    color: #ffffff;
}

.wf-tabbar {
    background: @wf_chrome;
    border-bottom: 1px solid @wf_line;
    min-height: 26px;
    padding: 4px 6px 0 6px;
}

.wf-tab {
    background: @wf_control;
    color: @wf_text_dim;
    border: 1px solid @wf_border;
    border-bottom: 0;
    border-radius: 6px 6px 0 0;
    min-height: 24px;
    padding: 3px 12px;
}

.wf-tab:hover {
    background: @wf_control_hover;
    color: #ffffff;
}

.wf-tab-active {
    background: @wf_raised;
    color: #ffffff;
    border-color: @wf_border;
}

.wf-tab-add {
    background: transparent;
    color: @wf_text_dim;
    border: 0;
    min-width: 26px;
    min-height: 24px;
    border-radius: 6px;
}

.wf-tab-add:hover {
    background: @wf_control_hover;
    color: #ffffff;
}

.wf-icon-button {
    background: @wf_control;
    color: @wf_text;
    border: 1px solid @wf_border;
    border-radius: 6px;
    min-height: 26px;
    min-width: 28px;
    padding: 0 6px;
    margin: 0;
}

.wf-icon-button:hover {
    background: @wf_control_hover;
}

.wf-icon-button:checked {
    background: @wf_accent;
    color: #ffffff;
    border-color: @wf_accent;
}

.wf-pathbar {
    background: @wf_raised;
    border-bottom: 1px solid @wf_line;
    min-height: 32px;
    padding: 0 6px;
}

/* Explorer's second toolbar row. Groups of buttons split by thin rules, with
   the view controls pushed to the right. */
.wf-commandbar {
    background: @wf_raised;
    border-bottom: 1px solid @wf_line;
    min-height: 34px;
    padding: 3px 6px;
}

.wf-command-divider {
    background: @wf_border;
    min-width: 1px;
    margin: 5px 5px;
}

.wf-address {
    background: transparent;
    color: @wf_text;
    border: 0;
    padding: 0 8px;
}

.wf-crumb {
    background: transparent;
    color: @wf_text_dim;
    border: 0;
    box-shadow: none;
    min-height: 22px;
    min-width: 0;
    padding: 0 6px;
    border-radius: 5px;
}

.wf-crumb:hover {
    background: @wf_control;
    color: #ffffff;
}

.wf-crumb-current {
    color: @wf_text;
    font-weight: 700;
}

.wf-crumb-sep {
    color: @wf_text_muted;
    padding: 0 1px;
}

.wf-address-box {
    background: transparent;
    border: 0;
    border-radius: 0;
    min-height: 26px;
}

.wf-search-scope {
    color: @wf_text_dim;
    font-size: 12px;
}

.wf-search {
    background: @wf_base;
    color: @wf_text;
    border: 1px solid @wf_border;
    border-radius: 6px;
    min-height: 26px;
}

.wf-main {
    background: @wf_base;
}

.wf-sidebar {
    background: @wf_raised;
    padding: 7px 7px;
}

.wf-sidebar row {
    border-radius: 6px;
    color: @wf_text_dim;
}

.wf-sidebar row:hover {
    background: @wf_control_hover;
}

.wf-sidebar row.wf-sidebar-active {
    background: @wf_accent;
}

.wf-sidebar row.wf-sidebar-active label {
    color: #ffffff;
}

.wf-sidebar-heading {
    color: @wf_text_muted;
    font-size: 11px;
    font-weight: 500;
    text-transform: uppercase;
}

.wf-sidebar-title {
    color: @wf_text;
    font-weight: 400;
}

.wf-sidebar-detail {
    color: @wf_text_muted;
    font-size: 11px;
}

progressbar.wf-space-bar.horizontal {
    margin-top: 3px;
    min-width: 0;
    min-height: 4px;
}

progressbar.wf-space-bar.horizontal trough {
    min-width: 0;
    min-height: 4px;
    border-radius: 2px;
    background: @wf_border;
}

progressbar.wf-space-bar.horizontal progress {
    min-height: 4px;
    border-radius: 2px;
    background: @wf_accent_hover;
}

.wf-sidebar row.wf-sidebar-active progressbar.wf-space-bar.horizontal trough {
    background: rgba(0, 0, 0, 0.35);
}

.wf-sidebar row.wf-sidebar-active progressbar.wf-space-bar.horizontal progress {
    background: #ffffff;
}

.wf-files-pane {
    background: @wf_base;
}

.wf-files-stack {
    background: @wf_base;
}

.wf-column-header {
    background: @wf_raised;
    color: @wf_text_dim;
    border-bottom: 1px solid @wf_line;
    min-height: 26px;
    padding: 0 10px;
}

.wf-column-header label {
    font-weight: 700;
    font-size: 12px;
}

/* Buttons carry default padding, which pushed each heading off the column of
   values beneath it. The heading must start exactly where its values start. */
.wf-column-header button {
    padding: 0;
    margin: 0;
    min-height: 0;
    min-width: 0;
    background: none;
    border: 0;
    box-shadow: none;
}

.wf-column-header button:hover {
    background: @wf_control;
}

/* The handle needs a 10px grab target but must READ as a hairline divider.
   Painting its background filled all 10px with near-black, which showed up as
   a black block between the headings. Paint a 1px border instead. */
.wf-column-resizer {
    background: transparent;
    border-left: 1px solid @wf_border;
    margin-top: 5px;
    margin-bottom: 5px;
}

.wf-column-resizer:hover {
    border-left-color: @wf_accent_hover;
}

.wf-empty-placeholder {
    color: @wf_text_muted;
    font-size: 13px;
    padding: 28px;
}

/* ONE hairline between panes, no gap, still easy to grab.
   History, so nobody reintroduces either bug:
   - It started as a 5px bar of near-black, AND the sidebar/preview each drew
     their own border against it: a ~6px black band with two lines in it.
   - Then transparent borders were used to widen the drag target. That failed:
     transparent shows whatever is BEHIND, which is the paned's own background,
     so a sliver of the file-list colour appeared past the line and bled over
     the lighter preview panel. The theme's own separator image also won over
     the background colour, painting the line light grey instead of near-black.
   Negative margin + equal padding is the fix: the widget's LAYOUT footprint
   stays 1px (so the panes sit flush against the line with nothing between
   them) while its INPUT area is 7px wide. background-clip keeps the paint in
   the 1px content box, and background-image: none stops the theme drawing its
   own handle over the top. */
paned > separator {
    background-color: @wf_line;
    background-image: none;
    background-clip: content-box;
    min-width: 1px;
    min-height: 1px;
    margin: 0 -3px;
    padding: 0 3px;
}

paned > separator:hover {
    background-color: @wf_accent_hover;
    background-image: none;
}

/* Scrollbars must not draw a second line next to the pane divider.
   GTK's default trough paints a full-height edge, so every pane boundary showed
   TWO verticals a few pixels apart: the trough at one x and the divider at
   another. Only the slider is visible now, and it floats over the content
   instead of reserving a lane of its own. */
scrollbar,
scrollbar trough {
    background: transparent;
    border: none;
    box-shadow: none;
    margin: 0;
    padding: 0;
}

/* Inset with margin, NOT a transparent border: a border is subtracted from the
   slider's minimum size, and GTK warned "slider reported min width -4". */
scrollbar slider {
    background: rgba(255, 255, 255, 0.16);
    border: none;
    border-radius: 6px;
    min-width: 8px;
    min-height: 28px;
    margin: 2px;
}

scrollbar slider:hover {
    background: rgba(255, 255, 255, 0.30);
}

scrollbar slider:active {
    background: @wf_accent;
}

.wf-list {
    background: @wf_base;
}

.wf-list row {
    border-radius: 0;
    /* Adwaita gives every list row 2px padding. That ring, plus the old margins
       on the row box, was dead space no file owned: pressing there started a
       selection box instead of picking the file up. The inset now lives inside
       the row box (.wf-file-row padding), so the whole line belongs to the file. */
    padding: 0;
    color: @wf_text;
    border-bottom: 1px solid rgba(255,255,255,0.025);
}

.wf-list row:hover {
    background: @wf_control_hover;
}

.wf-list row:selected {
    background: @wf_accent;
    color: #ffffff;
}

.wf-file-row {
    min-height: 26px;
    /* Same geometry as the old 2px row padding + 2px/10px margins. */
    padding: 4px 12px;
}

.wf-drag-badge {
    background: @wf_accent;
    color: #ffffff;
    border-radius: 8px;
    padding: 0 5px;
    font-size: 10px;
}

.wf-file-icon {
    min-width: 24px;
    min-height: 22px;
}

.wf-file-name {
    font-weight: 400;
    font-size: 15px;
}

.wf-file-meta {
    color: @wf_text_dim;
    font-size: 15px;
}

.wf-list row:selected .wf-file-meta {
    color: #ffffff;
}

.wf-list.wf-list-compact .wf-file-row {
    min-height: 24px;
}

.wf-list.wf-list-compact .wf-file-name,
.wf-list.wf-list-compact .wf-file-meta {
    font-size: 14px;
}

.wf-grid {
    background: @wf_base;
    padding: 12px;
}

.wf-grid-child {
    border-radius: 6px;
    color: @wf_text;
}

.wf-grid-child:hover {
    background: @wf_control_hover;
}

.wf-grid-child:selected {
    background: @wf_accent;
    color: #ffffff;
}

.wf-grid-item {
    min-width: 94px;
    min-height: 82px;
    padding: 6px;
}

.wf-preview-zoomed {
    font-size: 16px;
}

.wf-grid-name {
    font-size: 13px;
    font-weight: 400;
}

.wf-preview {
    background: @wf_raised;
}

.wf-picker-actions {
    background: @wf_raised;
    border-top: 1px solid @wf_line;
    padding: 7px 10px;
}

.wf-primary-button {
    background: @wf_accent;
    color: #ffffff;
    border: 1px solid @wf_accent;
    border-radius: 6px;
    min-height: 26px;
    min-width: 78px;
    padding: 0 14px;
    font-weight: 700;
}

.wf-primary-button:hover {
    background: @wf_accent_hover;
    border-color: @wf_accent_hover;
}

.wf-secondary-button {
    background: @wf_control;
    color: @wf_text;
    border: 1px solid @wf_border;
    border-radius: 6px;
    min-height: 26px;
    min-width: 78px;
    padding: 0 14px;
}

.wf-secondary-button:hover {
    background: @wf_control_hover;
}

.wf-preview-text {
    background: @wf_raised;
    color: @wf_text_dim;
    font-family: "Inter", "Cantarell", "Noto Sans", "DejaVu Sans", sans-serif;
    font-size: 14px;
}

.wf-preview-empty {
    color: @wf_text_muted;
    font-size: 14px;
}

.wf-status {
    background: @wf_chrome;
    color: @wf_text_dim;
    border-top: 1px solid @wf_line;
    font-size: 12px;
    padding: 6px 10px;
}

popover box {
    background: @wf_raised;
}

popover contents {
    background: @wf_raised;
    border: 1px solid @wf_border;
    border-radius: 8px;
    padding: 4px;
}

/* A menu row: fixed height, text hard left, shortcut hard right. Explorer's
   menu reads as a menu because every row is the same height and everything
   lines up on one left edge. */
.wf-menu-item {
    color: @wf_text;
    border-radius: 5px;
    padding: 0 10px;
    min-height: 28px;
    min-width: 200px;
}

/* Explorer highlights a menu row with a soft neutral, not a saturated accent.
   A full-strength blue on every hover made the menu feel noisy. */
.wf-menu-item:hover {
    background: @wf_control_hover;
    color: #ffffff;
}

/* Explorer's top strip: icon-only buttons for the actions you reach for
   constantly, sitting above the labelled list. */
.wf-menu-strip {
    padding: 2px 2px 4px 2px;
}

.wf-menu-tool {
    background: transparent;
    border: none;
    box-shadow: none;
    color: @wf_text_dim;
    border-radius: 5px;
    min-width: 34px;
    min-height: 30px;
    padding: 0;
    margin: 0;
}

.wf-menu-tool:hover {
    background: @wf_control_hover;
    color: #ffffff;
}

.wf-menu-icon {
    color: @wf_text_dim;
    -gtk-icon-size: 16px;
}

.wf-menu-item:hover .wf-menu-icon {
    color: #ffffff;
}

.wf-menu-accel {
    color: @wf_text_muted;
    font-size: 11px;
}

.wf-menu-item:hover .wf-menu-accel {
    color: rgba(255, 255, 255, 0.85);
}

/* Tight rules that group actions, not big gaps that pull the list apart. */
.wf-menu-sep {
    background: @wf_border;
    min-height: 1px;
    margin: 4px 6px;
}
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone)]
struct FileClipboard {
    mode: ClipboardMode,
    paths: Vec<String>,
}

#[derive(Clone)]
enum UndoAction {
    Created {
        paths: Vec<String>,
    },
    Renamed {
        from: String,
        to: String,
    },
    BatchRenamed {
        items: Vec<ResolvedItem>,
    },
    Pasted {
        items: Vec<ResolvedItem>,
        was_copy: bool,
    },
    Trashed {
        paths: Vec<String>,
    },
}

struct JobOutcome {
    message: String,
    undo: Option<UndoAction>,
    refresh_to: Option<String>,
}

impl JobOutcome {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            undo: None,
            refresh_to: None,
        }
    }

    fn undo(message: impl Into<String>, undo: UndoAction) -> Self {
        Self {
            message: message.into(),
            undo: Some(undo),
            refresh_to: None,
        }
    }

    fn navigate(message: impl Into<String>, path: String) -> Self {
        Self {
            message: message.into(),
            undo: None,
            refresh_to: Some(path),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Details,
    Compact,
    Grid,
}

#[derive(Clone, Copy)]
enum ZoomTarget {
    Files,
    Preview,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Bookmark {
    name: String,
    path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum SortKey {
    Name,
    Size,
    Kind,
    Modified,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppConfig {
    favorites: Vec<Bookmark>,
    recents: Vec<Bookmark>,
    show_hidden: bool,
    view_mode: String,
    search_in_files: bool,
    sort_key: SortKey,
    sort_ascending: bool,
}

#[derive(Clone, Debug)]
struct PickerConfig {
    multiple: bool,
    directory: bool,
    starting_dir: Option<String>,
    filter: Option<String>,
    window: Option<PickerWindow>,
}

#[derive(Clone, Debug)]
struct PickerState {
    config: PickerConfig,
    out_file: String,
}

#[derive(Clone, Copy, Debug)]
struct PickerWindow {
    width: i32,
    height: i32,
}

fn parse_picker_size(value: &str) -> Option<PickerWindow> {
    let normalized = value.trim().replace(',', "x");
    let mut parts = normalized
        .split('x')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let width = parts.next()?.parse::<i32>().ok()?;
    let height = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(PickerWindow {
        width: width.clamp(640, 1400),
        height: height.clamp(420, 1000),
    })
}

fn parse_picker_window(value: &str) -> Option<PickerWindow> {
    let parts: Vec<_> = value
        .trim()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        [width, height] => parse_picker_size(&format!("{width}x{height}")),
        [_x, _y, width, height] => parse_picker_size(&format!("{width}x{height}")),
        _ => None,
    }
}

fn parse_picker_state() -> Option<PickerState> {
    let mut args = std::env::args().skip(1);
    let mut pick = false;
    let mut multiple = false;
    let mut directory = false;
    let mut starting_dir = None;
    let mut filter = None;
    let mut out_file = None;
    let mut window = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pick" => pick = true,
            "--multiple" => multiple = true,
            "--directory" => directory = true,
            "--out" => out_file = args.next(),
            "--starting-dir" => starting_dir = args.next(),
            "--filter" => filter = args.next(),
            "--picker-size" => {
                if let Some(value) = args.next() {
                    window = parse_picker_size(&value);
                }
            }
            "--picker-window" => {
                if let Some(value) = args.next() {
                    window = parse_picker_window(&value);
                }
            }
            _ => {}
        }
    }

    if !pick {
        return None;
    }

    Some(PickerState {
        config: PickerConfig {
            multiple,
            directory,
            starting_dir,
            filter,
            window,
        },
        out_file: out_file?,
    })
}

fn picker_filter_allows(picker: Option<&PickerState>, entry: &Entry) -> bool {
    let Some(picker) = picker else {
        return true;
    };
    let Some(filter) = picker.config.filter.as_deref() else {
        return true;
    };
    match filter {
        "images" => entry.is_dir || entry.is_image,
        _ => true,
    }
}

fn picker_write_and_exit(picker: &PickerState, paths: Vec<String>, code: i32) -> ! {
    let output = if code == 0 {
        paths
            .into_iter()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let output = if output.is_empty() {
        output
    } else {
        format!("{output}\n")
    };
    let _ = fs::write(&picker.out_file, output);
    std::process::exit(code);
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            favorites: default_favorites(),
            recents: Vec::new(),
            show_hidden: false,
            view_mode: "details".to_string(),
            search_in_files: false,
            sort_key: SortKey::Name,
            sort_ascending: true,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("wayfinder")
            .join("native-config.json")
    })
}

fn load_config() -> AppConfig {
    config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn read_config_at(path: &Path) -> AppConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Reads the config as it is on disk NOW, applies one change, writes it back.
/// Every window shares the file, so a window may only change the fields it
/// owns; rewriting a stale full snapshot would undo another window's changes.
fn update_config_at(path: &Path, change: impl FnOnce(&mut AppConfig)) -> AppConfig {
    let mut config = read_config_at(path);
    change(&mut config);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(&config) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, raw).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
    config
}

fn update_config(change: impl FnOnce(&mut AppConfig)) -> Option<AppConfig> {
    config_path().map(|path| update_config_at(&path, change))
}

// Each view control persists only the field the user changed, so a window
// holding stale values never writes them over another window's change.
fn config_show_hidden(config: &mut AppConfig, value: bool) {
    config.show_hidden = value;
}

fn config_search_in_files(config: &mut AppConfig, value: bool) {
    config.search_in_files = value;
}

fn config_view_mode(config: &mut AppConfig, mode: ViewMode) {
    config.view_mode = view_mode_id(mode).to_string();
}

fn config_sort(config: &mut AppConfig, key: SortKey, ascending: bool) {
    config.sort_key = key;
    config.sort_ascending = ascending;
}

fn default_favorites() -> Vec<Bookmark> {
    let mut out = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let projects = PathBuf::from(&home).join("Projects");
        if projects.is_dir() {
            out.push(Bookmark {
                name: "Projects".to_string(),
                path: projects.to_string_lossy().to_string(),
            });
        }
    }
    out
}

fn view_mode_from_id(value: &str) -> ViewMode {
    match value {
        "compact" => ViewMode::Compact,
        "grid" | "icons" => ViewMode::Grid,
        _ => ViewMode::Details,
    }
}

fn view_mode_id(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Details => "details",
        ViewMode::Compact => "compact",
        ViewMode::Grid => "grid",
    }
}

fn bookmark_for_path(path: &str) -> Bookmark {
    Bookmark {
        name: tab_label(path),
        path: path.to_string(),
    }
}

fn push_recent(recents: &mut Vec<Bookmark>, path: &str) {
    if !Path::new(path).is_dir() {
        return;
    }
    recents.retain(|bookmark| bookmark.path != path);
    recents.insert(0, bookmark_for_path(path));
    recents.truncate(12);
}

fn add_favorite(favorites: &mut Vec<Bookmark>, path: &str) {
    if !favorites.iter().any(|favorite| favorite.path == path) {
        favorites.push(bookmark_for_path(path));
    }
}

fn remove_favorite(favorites: &mut Vec<Bookmark>, path: &str) {
    favorites.retain(|favorite| favorite.path != path);
}

fn is_favorite(state: &AppState, path: &str) -> bool {
    state.favorites.iter().any(|favorite| favorite.path == path)
}

fn add_favorite_path(state: &Rc<RefCell<AppState>>, ui: &Ui, path: String) {
    if !Path::new(&path).is_dir() {
        ui.status
            .set_label("Only folders can be added to Favorites");
        return;
    }
    {
        let mut state_ref = state.borrow_mut();
        if !is_favorite(&state_ref, &path) {
            match update_config(|config| add_favorite(&mut config.favorites, &path)) {
                Some(config) => state_ref.favorites = config.favorites,
                None => add_favorite(&mut state_ref.favorites, &path),
            }
        }
    }
    rebuild_sidebar(state, ui);
    ui.status.set_label("Added to Favorites");
}

fn remove_favorite_path(state: &Rc<RefCell<AppState>>, ui: &Ui, path: String) {
    {
        let mut state_ref = state.borrow_mut();
        match update_config(|config| remove_favorite(&mut config.favorites, &path)) {
            Some(config) => state_ref.favorites = config.favorites,
            None => remove_favorite(&mut state_ref.favorites, &path),
        }
    }
    rebuild_sidebar(state, ui);
    ui.status.set_label("Removed from Favorites");
}

/// True when the shared config on disk holds no recents. Read from disk, not
/// from this window, because another window may have added visits since.
fn recents_are_empty(state: &Rc<RefCell<AppState>>) -> bool {
    match config_path() {
        Some(path) => read_config_at(&path).recents.is_empty(),
        None => state.borrow().recents.is_empty(),
    }
}

/// Clears only `AppConfig.recents`, then makes every live window match the
/// committed result, so a stale window cannot write the old entries back.
fn clear_recents(ui: &Ui) {
    let committed = update_config(|config| config.recents.clear()).map(|config| config.recents);
    let windows = WINDOWS.with(|windows| windows.borrow().windows());
    for (window_state, window_ui) in windows {
        window_state.borrow_mut().recents = committed.clone().unwrap_or_default();
        rebuild_sidebar(&window_state, &window_ui);
    }
    ui.status.set_label("Recent folders cleared");
}

/// One tab: its current folder plus its own back/forward history. `id` is stable
/// for the tab's lifetime, so an async listing can never land in another tab.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TabState {
    id: u64,
    path: String,
    history: Vec<String>,
    index: usize,
}

impl TabState {
    fn new(id: u64, path: String) -> Self {
        Self { id, history: vec![path.clone()], path, index: 0 }
    }

    /// Ordinary navigation: drop only this tab's forward branch, then append if
    /// the target differs from the current entry.
    fn record(&mut self, path: &str) {
        self.path = path.to_string();
        if self.history.get(self.index).map(|c| c != path).unwrap_or(true) {
            self.history.truncate(self.index + 1);
            self.history.push(path.to_string());
            self.index = self.history.len() - 1;
        }
    }

    fn can_back(&self) -> bool {
        self.index > 0
    }

    fn can_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }
}

/// The window's tabs. All history reads and writes go through the active tab.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TabSet {
    tabs: Vec<TabState>,
    active: usize,
    next_id: u64,
}

impl TabSet {
    fn new(path: String) -> Self {
        Self { tabs: vec![TabState::new(0, path)], active: 0, next_id: 1 }
    }

    fn active(&self) -> &TabState {
        &self.tabs[self.active]
    }

    fn active_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active]
    }

    /// Adds a tab with one history entry and makes it active.
    fn open(&mut self, path: String) {
        self.tabs.push(TabState::new(self.next_id, path));
        self.next_id += 1;
        self.active = self.tabs.len() - 1;
    }

    fn switch(&mut self, index: usize) -> Option<String> {
        if index >= self.tabs.len() {
            return None;
        }
        self.active = index;
        Some(self.active().path.clone())
    }

    /// Closes the active tab; returns the surviving active tab's path.
    fn close_active(&mut self) -> Option<String> {
        if self.tabs.len() <= 1 {
            return None;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        Some(self.active().path.clone())
    }

    /// Target index and path one step back / forward in the active tab. The
    /// cursor only moves when that listing succeeds.
    fn back_target(&self) -> Option<(usize, String)> {
        let tab = self.active();
        tab.can_back().then(|| (tab.index - 1, tab.history[tab.index - 1].clone()))
    }

    fn forward_target(&self) -> Option<(usize, String)> {
        let tab = self.active();
        tab.can_forward().then(|| (tab.index + 1, tab.history[tab.index + 1].clone()))
    }
}

/// How a finished listing updates the active tab's history.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NavMode {
    Record,
    Keep,
    Cursor(usize),
}

#[derive(Clone)]
struct AppState {
    inline_rename: Option<InlineRename>,
    /// A path to select and scroll to once the next listing is on screen.
    reveal_path: Option<String>,
    /// Last plain click on a Name label: path and when, for slow-second-click.
    last_name_click: Option<(String, std::time::Instant)>,
    activation_gate: Rc<RefCell<ActivationGate>>,
    cwd: String,
    tabs: TabSet,
    entries: Vec<Entry>,
    visible_entries: Vec<Entry>,
    /// A path forced to the very top of the listing regardless of sort — set when
    /// the app is launched pointing at a file (download-and-open flow).
    pinned_top: Option<String>,
    filter_text: String,
    show_hidden: bool,
    search_in_files: bool,
    view_mode: ViewMode,
    icon_zoom: f64,
    preview_zoom: f64,
    sort_key: SortKey,
    sort_ascending: bool,
    sidebar_active_key: Option<String>,
    favorites: Vec<Bookmark>,
    recents: Vec<Bookmark>,
    tree_expanded: HashSet<String>,
    tree_children: HashMap<String, Vec<(String, String, bool)>>,
    typeahead: String,
    typeahead_at: Instant,
    clipboard: Option<FileClipboard>,
    undo_stack: Vec<UndoAction>,
    picker: Option<PickerState>,
    /// Every navigation owns one cancellable folder load. A late response must
    /// never overwrite the folder the user navigated to afterwards.
    listing_generation: u64,
    listing_cancel: Option<Arc<AtomicBool>>,
    /// Keeps the active local-directory monitor alive and invalidates debounced
    /// notifications from the folder we just left.
    directory_monitor: Option<gio::FileMonitor>,
    monitor_generation: u64,
    /// Realized Size/Modified labels by path, kept by the list bind/unbind so a
    /// metadata-only refresh can update text without touching the rows.
    meta_labels: HashMap<String, (glib::WeakRef<gtk::Label>, glib::WeakRef<gtk::Label>)>,
    thumbnail_queue: Option<Rc<RefCell<ThumbnailQueue>>>,
    /// Last drive scan (with space), filled off the window thread. The sidebar and
    /// row activation read only this, never a fresh scan.
    drives: Vec<Drive>,
    drive_refresh_in_flight: bool,
    /// A refresh was asked for while one ran; run once more when it lands.
    drive_refresh_again: bool,
    drive_scan: DriveScan,
    /// The one newest guide request for this window (never queued).
    #[cfg(feature = "guide")]
    guide_pending: GuideRequest,
    /// The one live widget the pending known request names, held weakly so a
    /// closed dynamic popover is never kept alive.
    #[cfg(feature = "guide")]
    guide_target: Option<glib::WeakRef<gtk::Widget>>,
    /// The ring and label currently drawn for `guide_target`, if any.
    #[cfg(feature = "guide")]
    guide_visual: Option<GuideVisual>,
    #[cfg(feature = "guide")]
    guide_monitor: Option<gio::FileMonitor>,
    /// The directory `guide_monitor` watches (the guide directory itself once promoted).
    #[cfg(feature = "guide")]
    guide_watch_dir: Option<PathBuf>,
}

struct ThumbnailJob {
    path: String,
    picture: gtk::Picture,
    width: i32,
    height: i32,
}

struct ThumbnailQueue {
    cancelled: gio::Cancellable,
    pending: VecDeque<ThumbnailJob>,
    active: usize,
}

#[derive(Clone)]
struct Ui {
    window: adw::ApplicationWindow,
    tabbar: gtk::Box,
    back: gtk::Button,
    forward: gtk::Button,
    hidden: gtk::ToggleButton,
    files_stack: gtk::Stack,
    column_header: gtk::Box,
    columns: ColumnSizing,
    path_stack: gtk::Stack,
    path_crumbs: gtk::Box,
    path_entry: gtk::Entry,
    search: gtk::SearchEntry,
    sidebar: gtk::ListBox,
    list: gtk::ListView,
    list_store: gio::ListStore,
    list_selection: gtk::MultiSelection,
    list_placeholder: gtk::Label,
    list_scroll: gtk::ScrolledWindow,
    grid: gtk::FlowBox,
    preview_stack: gtk::Stack,
    preview_scroll: gtk::ScrolledWindow,
    file_preview: gtk::Paned,
    preview_toggle: gtk::ToggleButton,
    /// Divider position remembered while the preview pane is hidden.
    preview_saved_position: Rc<std::cell::Cell<i32>>,
    preview_picture: gtk::Picture,
    preview_media_picture: gtk::Picture,
    preview_media_controls: gtk::MediaControls,
    preview: gtk::Label,
    status: gtk::Label,
    picker_open: Option<gtk::Button>,
}

/// The interface Chrome, Brave, Firefox and most GTK/Qt apps call to say
/// "show me this exact file", rather than just "open this folder". Without a
/// provider on the bus, browsers silently fall back to opening the parent
/// directory and the download is never highlighted.
const FILE_MANAGER1_XML: &str = r#"
<node>
  <interface name='org.freedesktop.FileManager1'>
    <method name='ShowFolders'>
      <arg type='as' name='URIs' direction='in'/>
      <arg type='s' name='StartupId' direction='in'/>
    </method>
    <method name='ShowItems'>
      <arg type='as' name='URIs' direction='in'/>
      <arg type='s' name='StartupId' direction='in'/>
    </method>
    <method name='ShowItemProperties'>
      <arg type='as' name='URIs' direction='in'/>
      <arg type='s' name='StartupId' direction='in'/>
    </method>
  </interface>
</node>
"#;

/// Publish org.freedesktop.FileManager1 so "Show in folder" lands on the file.
///
/// Every method is routed back through `app.open`, which already navigates to
/// the parent and selects the item, so there is exactly one code path for
/// "reveal this file" no matter whether it arrived from the bus, the command
/// line or a desktop launcher.
fn export_file_manager1(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        eprintln!("wayfinder: no session bus; 'Show in folder' will not highlight files");
        return;
    };
    let Ok(node) = gio::DBusNodeInfo::for_xml(FILE_MANAGER1_XML) else {
        return;
    };
    let Some(interface) = node.lookup_interface("org.freedesktop.FileManager1") else {
        return;
    };

    let app_for_calls = app.clone();
    let registered = connection
        .register_object("/org/freedesktop/FileManager1", &interface)
        .method_call(move |_, _, _, _, method, params, invocation| {
            // Every method takes (as URIs, s StartupId).
            let uris: Vec<String> = params
                .try_child_get::<Vec<String>>(0)
                .ok()
                .flatten()
                .unwrap_or_default();
            let files: Vec<gio::File> = uris.iter().map(|uri| gio::File::for_uri(uri)).collect();
            if !files.is_empty() {
                // All three methods take the same route because `connect_open`
                // already branches on what the path IS: a directory opens
                // directly, a file opens its parent with the file selected.
                // That is exactly ShowFolders vs ShowItems semantics.
                let _ = method;
                app_for_calls.open(&files, "");
            }
            // The interface returns nothing; reply so the caller is not left
            // waiting for its 25s D-Bus timeout.
            invocation.return_value(None);
        })
        .build();
    if let Err(error) = registered {
        eprintln!("wayfinder: could not export FileManager1: {error}");
        return;
    }

    // Claim the well-known name last, so the object is already answering by the
    // time anything can address us.
    gio::bus_own_name_on_connection(
        &connection,
        "org.freedesktop.FileManager1",
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, _| {},
    );
}

// Inert guide-only markers: kept in `--features guide` builds so the release
// ship check can tell them apart from a default build. No I/O, no state.
#[cfg(feature = "guide")]
#[inline(never)]
fn guide_markers_inert() -> usize {
    const MARKERS: [&str; 5] = [
        "/.local/share/crew/guide/",
        "WF_GUIDE_FILE",
        "WAYFINDER_GUIDE_ONLY_V1",
        "WAYFINDER_GUIDE_CIRCLE_V1",
        "WAYFINDER_GUIDE_ARROW_V1",
    ];
    std::hint::black_box(MARKERS).iter().map(|m| m.len()).sum()
}

/// One guide request from the transport file (guide builds only). Names are
/// classified by exact key equality against the tracked registry; an unknown
/// name is kept exactly for the later writeback round.
#[cfg(feature = "guide")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum GuideRequest {
    Clear,
    Known { name: String, label: String },
    Unknown { name: String, label: String },
}

#[cfg(feature = "guide")]
const GUIDE_REGISTRY: &str = include_str!("guide-names.json");

#[cfg(feature = "guide")]
fn guide_file_path() -> PathBuf {
    match std::env::var("WF_GUIDE_FILE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".local/share/crew/guide/wayfinder.json"),
    }
}

#[cfg(feature = "guide")]
fn guide_parse(text: &str) -> GuideRequest {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return GuideRequest::Clear;
    };
    let name = value.get("highlight").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return GuideRequest::Clear;
    }
    let label = value.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let known = serde_json::from_str::<serde_json::Value>(GUIDE_REGISTRY)
        .ok()
        .and_then(|r| r.as_object().map(|o| o.contains_key(name)))
        .unwrap_or(false);
    let name = name.to_string();
    if known {
        GuideRequest::Known { name, label }
    } else {
        GuideRequest::Unknown { name, label }
    }
}

/// The one name source: a widget is named only with a key that exists in the
/// registry, byte for byte.
#[cfg(feature = "guide")]
fn guide_name(widget: &impl IsA<gtk::Widget>, key: &str) {
    let known = serde_json::from_str::<serde_json::Value>(GUIDE_REGISTRY)
        .ok()
        .and_then(|r| r.as_object().map(|o| o.contains_key(key)))
        .unwrap_or(false);
    if known {
        widget.set_widget_name(key);
    }
}

/// Names a drive-menu control for the two fixture drives only: the non-removable
/// `drive` row "Launch Box SSD 2TB" (crucial) and any removable `drive` row (stick).
#[cfg(feature = "guide")]
fn guide_name_drive(widget: &impl IsA<gtk::Widget>, action: &str, drive: &Drive) {
    let tag = if drive.kind != "drive" {
        return;
    } else if drive.removable {
        "stick"
    } else if drive.name == "Launch Box SSD 2TB" {
        "crucial"
    } else {
        return;
    };
    guide_name(widget, &format!("drive-{action}-{tag}"));
}

/// A missing or unreadable file clears, like malformed JSON.
#[cfg(feature = "guide")]
fn guide_read(path: &Path) -> GuideRequest {
    fs::read_to_string(path).map(|t| guide_parse(&t)).unwrap_or(GuideRequest::Clear)
}

/// Every live widget under `root` (popovers included) named exactly `name`.
#[cfg(feature = "guide")]
fn guide_find(root: &gtk::Widget, name: &str, out: &mut Vec<gtk::Widget>) {
    if root.widget_name() == name {
        out.push(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        guide_find(&c, name, out);
        child = c.next_sibling();
    }
}

/// Binds the pending request to its one real widget. Clear and unknown requests
/// leave no target; a known one binds only when exactly one live widget carries
/// the name (none: stays pending; several: fails closed).
#[cfg(feature = "guide")]
fn guide_resolve(state: &Rc<RefCell<AppState>>) {
    let request = state.borrow().guide_pending.clone();
    let target = match request {
        GuideRequest::Known { name, .. } => ui_for_state(state).and_then(|ui| {
            let mut found = Vec::new();
            guide_find(ui.window.upcast_ref(), &name, &mut found);
            (found.len() == 1).then(|| found[0].downgrade())
        }),
        _ => None,
    };
    state.borrow_mut().guide_target = target;
    guide_paint(state);
}

/// Ring on the target (a CSS outline, so nothing moves) plus one label surface
/// anchored to it. The label is a non-modal popover that cannot be targeted or
/// focused, so it never grabs input or dismisses a containing menu.
#[cfg(feature = "guide")]
#[derive(Clone)]
struct GuideVisual {
    target: glib::WeakRef<gtk::Widget>,
    mark: GuideMark,
    label: gtk::Popover,
    /// The clear-on-use callback on the target button, so it can be removed.
    hook: Rc<RefCell<Option<GuideHook>>>,
}

#[cfg(feature = "guide")]
type GuideHook = (glib::WeakRef<gtk::Button>, glib::SignalHandlerId);

#[cfg(feature = "guide")]
fn guide_release_hook(hook: Option<GuideHook>) {
    if let Some((button, id)) = hook {
        if let Some(button) = button.upgrade() {
            button.disconnect(id);
        }
    }
}

/// D-75: the one fixture item the tutorial points at: the file `open-me.txt` directly inside a
/// folder named `Throwaways`. Nothing else, and no other file name, is ever named.
#[cfg(feature = "guide")]
const GUIDE_OPEN_FILE: &str = "open-file-other-app";

#[cfg(feature = "guide")]
fn guide_is_open_file_fixture(entry: &Entry) -> bool {
    let path = Path::new(&entry.path);
    !entry.is_dir
        && path.file_name().is_some_and(|n| n == "open-me.txt")
        && path.parent().and_then(Path::file_name).is_some_and(|n| n == "Throwaways")
}

#[cfg(feature = "guide")]
fn guide_waits_for_open_file(state: &Rc<RefCell<AppState>>) -> bool {
    matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == GUIDE_OPEN_FILE)
}

/// One post-layout resolve after the fixture item binds or goes away: the item is allocated only
/// after its view lays out. Nothing runs when the pending request is not this one.
#[cfg(feature = "guide")]
fn guide_resolve_open_file_later(state: &Rc<RefCell<AppState>>) {
    if !guide_waits_for_open_file(state) {
        return;
    }
    let state = state.clone();
    glib::idle_add_local_once(move || {
        if guide_waits_for_open_file(&state) {
            guide_resolve(&state);
        }
    });
}

/// Observes the existing activation route: opening the bound fixture item clears only this
/// request. It never opens anything itself; any other item leaves the request alone.
#[cfg(feature = "guide")]
fn guide_note_opened(state: &Rc<RefCell<AppState>>, entry: &Entry) {
    if !guide_is_open_file_fixture(entry) || !guide_waits_for_open_file(state) {
        return;
    }
    if !state.borrow().guide_target.as_ref().is_some_and(|t| t.upgrade().is_some()) {
        return;
    }
    {
        let mut st = state.borrow_mut();
        st.guide_pending = GuideRequest::Clear;
        st.guide_target = None;
    }
    guide_unpaint(state);
}

/// D-76: the Devices heading, the one target for the drive space words and bars.
#[cfg(feature = "guide")]
const GUIDE_DRIVE_SPACE: &str = "drive-space-bars";

#[cfg(feature = "guide")]
fn guide_waits_for_drive_space(state: &Rc<RefCell<AppState>>) -> bool {
    matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == GUIDE_DRIVE_SPACE)
}

/// One post-layout resolve after a sidebar rebuild: the fresh heading is allocated only after the
/// sidebar lays out. Nothing runs when the pending request is not this one.
#[cfg(feature = "guide")]
fn guide_resolve_drive_space_later(state: &Rc<RefCell<AppState>>) {
    if !guide_waits_for_drive_space(state) {
        return;
    }
    let state = state.clone();
    glib::idle_add_local_once(move || {
        if guide_waits_for_drive_space(&state) {
            guide_resolve(&state);
        }
    });
}

/// D-77: the one file-view surface, named only while browsing the synthetic-or-real Launch Box SSD
/// mount tree. Nothing else is ever named with this key.
#[cfg(feature = "guide")]
const GUIDE_CRUCIAL_BACK: &str = "drive-mounted-again-crucial";

/// Exactly one cached, non-removable `drive` called `Launch Box SSD 2TB` with an absolute
/// mountpoint, and the current folder is that mountpoint or below it (whole path components).
#[cfg(feature = "guide")]
fn guide_crucial_context(drives: &[Drive], cwd: &str) -> bool {
    // Absolute and free of `..`: a parent-traversal path could lexically match while leaving the mount.
    // Harmless `.` spellings are already normalized away by the path components.
    let plain = |p: &str| {
        let p = Path::new(p);
        p.is_absolute() && p.components().all(|c| !matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir))
    };
    let mut eligible = drives.iter().filter(|d| {
        d.kind == "drive" && !d.removable && d.name == "Launch Box SSD 2TB" && plain(&d.mountpoint)
    });
    match (eligible.next(), eligible.next()) {
        (Some(d), None) => plain(cwd) && Path::new(cwd).starts_with(&d.mountpoint),
        _ => false,
    }
}

thread_local! {
    static GUIDE_RETRIED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A target that was not yet allocated when it was painted: resolve once more right after the next
/// frame is painted, when its size is known. One retry per unallocated paint, never a poll.
#[cfg(feature = "guide")]
fn guide_retry_after_paint(state: &Rc<RefCell<AppState>>, target: &gtk::Widget) {
    if GUIDE_RETRIED.with(|r| r.replace(true)) {
        return;
    }
    let Some(clock) = target.frame_clock() else {
        GUIDE_RETRIED.with(|r| r.set(false));
        return;
    };
    let state = state.clone();
    let id = Rc::new(RefCell::new(None::<glib::SignalHandlerId>));
    let id_in = id.clone();
    *id.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
        if let Some(id) = id_in.borrow_mut().take() {
            clock.disconnect(id);
        }
        let state = state.clone();
        glib::idle_add_local_once(move || {
            GUIDE_RETRIED.with(|r| r.set(false));
            if guide_waits_for_drive_space(&state)
                || matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == GUIDE_CRUCIAL_BACK)
            {
                guide_resolve(&state);
            }
        });
    }));
    target.queue_resize();
}

/// Keeps the guide-only name on the file view in step with navigation and the cached drive list.
/// A change re-resolves once after layout, only when this request is the pending one, so another
/// tutorial's visual is never torn down or replaced.
#[cfg(feature = "guide")]
fn guide_sync_crucial(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let eligible = {
        let st = state.borrow();
        guide_crucial_context(&st.drives, &st.cwd)
    };
    let named = ui.files_stack.widget_name() == GUIDE_CRUCIAL_BACK;
    if eligible != named {
        if eligible {
            guide_name(&ui.files_stack, GUIDE_CRUCIAL_BACK);
        } else {
            ui.files_stack.set_widget_name("");
        }
    }
    let waiting = matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == GUIDE_CRUCIAL_BACK);
    if !waiting || (eligible == named && (!eligible || state.borrow().guide_visual.is_some())) {
        return;
    }
    let state = state.clone();
    glib::idle_add_local_once(move || {
        if matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == GUIDE_CRUCIAL_BACK) {
            guide_resolve(&state);
        }
    });
}

/// Connects the one clear-on-use callback to the painted button. It clears only
/// while the activated button is still this window's bound target, so a stale,
/// duplicate, unresolved or other-window control can never clear the request.
#[cfg(feature = "guide")]
fn guide_connect_use(state: &Rc<RefCell<AppState>>, target: &gtk::Widget) -> Option<GuideHook> {
    if std::env::var("RED").is_ok() { return None; }
    let button = target.downcast_ref::<gtk::Button>()?;
    let weak = Rc::downgrade(state);
    let id = button.connect_clicked(move |used| {
        let Some(state) = weak.upgrade() else { return };
        let bound = state.borrow().guide_target.as_ref().and_then(|t| t.upgrade());
        if bound.as_ref() != Some(used.upcast_ref::<gtk::Widget>())
            || !matches!(state.borrow().guide_pending, GuideRequest::Known { .. })
        {
            return;
        }
        {
            let mut st = state.borrow_mut();
            st.guide_pending = GuideRequest::Clear;
            st.guide_target = None;
        }
        guide_unpaint(&state);
    });
    Some((button.downgrade(), id))
}

/// How a registry entry wants its target marked. Chosen only by the entry's own
/// `mark` field, never from the target's size or widget type. An entry whose mark
/// is missing or is neither value gets no mark at all.
#[cfg(feature = "guide")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuideMark {
    Circle,
    Arrow,
}

#[cfg(feature = "guide")]
fn guide_mark_for(name: &str) -> Option<GuideMark> {
    let mark = serde_json::from_str::<serde_json::Value>(GUIDE_REGISTRY)
        .ok()
        .and_then(|r| r.get(name)?.get("mark")?.as_str().map(str::to_owned));
    match mark.as_deref() {
        Some("circle") => Some(GuideMark::Circle),
        Some("arrow") => Some(GuideMark::Arrow),
        _ => None,
    }
}

// The circle is a target-side stroke: a fully rounded 4 px magenta outline pulled
// 3 px inside the control's own box, so it covers the edge, leaves the centre
// clear, takes no input and changes no allocation. The arrow surface is drawn
// bare: no popover chrome, only the magenta polygon and its white label.
#[cfg(feature = "guide")]
const GUIDE_CSS: &str = ".wf-guide-circle { outline: 4px solid #ff2fd0; outline-offset: -3px; border-radius: 9999px; } \
    popover.wf-guide-label > contents { background: #ff2fd0; color: #ffffff; padding: 2px 8px; border-radius: 6px; } \
    popover.wf-guide-label label { font-weight: 700; } \
    popover.wf-guide-arrow, popover.wf-guide-arrow > contents, popover.wf-guide-arrow > contents > * { \
        background: none; background-color: transparent; box-shadow: none; border: none; border-width: 0; \
        padding: 0; margin: 0; border-radius: 0; outline: none; } \
    popover.wf-guide-arrow label { color: #ffffff; }";

// Arrow drawing geometry, in pixels.
#[cfg(feature = "guide")]
const GUIDE_ARROW_H: f64 = 40.0; // head height
#[cfg(feature = "guide")]
const GUIDE_ARROW_SHAFT: f64 = 24.0; // shaft height: broad, 60% of the head
#[cfg(feature = "guide")]
const GUIDE_ARROW_HEAD: f64 = 22.0; // head length
#[cfg(feature = "guide")]
const GUIDE_ARROW_PAD: i32 = 10; // shaft padding around the label
#[cfg(feature = "guide")]
const GUIDE_ARROW_OVERLAP: i32 = 4; // the tip reaches this far over the target edge

/// The arrow polygon for a `w` x `h` surface: a triangular head of `head` length
/// spanning the full height, then a shaft of `shaft` height to the far edge. The
/// tip is on the left edge when `tip_left`, else on the right.
#[cfg(feature = "guide")]
fn guide_arrow_polygon(w: f64, h: f64, head: f64, shaft: f64, tip_left: bool) -> [(f64, f64); 7] {
    let (t, b) = ((h - shaft) / 2.0, (h + shaft) / 2.0);
    let pts = [(0.0, h / 2.0), (head, 0.0), (head, t), (w, t), (w, b), (head, b), (head, h)];
    pts.map(|(x, y)| (if tip_left { x } else { w - x }, y))
}

/// Which way the placed arrow points: tip left when GTK put the surface to the
/// right of the target, tip right when it flipped to the left. Read from the
/// placed popup position, so a side flip can never make it point away.
#[cfg(feature = "guide")]
fn guide_arrow_tip_left(pop: &gtk::Popover, target: &gtk::Widget) -> Option<bool> {
    if !pop.is_mapped() || pop.root().is_none() || target.root().is_none() {
        return None;
    }
    let native = pop.native()?;
    let surface = native.surface()?;
    let popup = surface.downcast_ref::<gdk::Popup>()?;
    let pop_centre = popup.position_x() as f64 + surface.width() as f64 / 2.0;
    let target_native = target.native()?;
    let b = target.compute_bounds(&target_native)?;
    let (tx, _) = target_native.surface_transform();
    Some(pop_centre > tx + b.x() as f64 + b.width() as f64 / 2.0)
}

/// One arrow surface anchored to `target`: a non-modal popover whose child is the
/// arrow drawing with its label on top. Nothing in it can be targeted or focused,
/// and its surface gets an empty input region so a real pointer over the tip
/// reaches the row underneath.
#[cfg(feature = "guide")]
fn guide_arrow_popover(target: &gtk::Widget, label: &str) -> gtk::Popover {
    let text = gtk::Label::new(Some(label));
    text.set_can_target(false);
    text.set_focusable(false);
    text.set_halign(gtk::Align::Start);
    text.set_valign(gtk::Align::Center);
    let tip_left = Rc::new(std::cell::Cell::new(true));
    // Label margins and the tip overlap follow the side the arrow points from.
    let place = |text: &gtk::Label, tip_left: bool| {
        let (near, far) = (GUIDE_ARROW_HEAD as i32 + GUIDE_ARROW_PAD, GUIDE_ARROW_PAD);
        text.set_margin_start(if tip_left { near } else { far });
        text.set_margin_end(if tip_left { far } else { near });
    };
    place(&text, true);
    let area = gtk::DrawingArea::new();
    area.set_content_height(GUIDE_ARROW_H as i32);
    area.set_can_target(false);
    area.set_focusable(false);
    let overlay = gtk::Overlay::new();
    overlay.set_can_target(false);
    overlay.set_focusable(false);
    overlay.set_child(Some(&area));
    overlay.add_overlay(&text);
    overlay.set_measure_overlay(&text, true);
    let pop = gtk::Popover::new();
    pop.set_autohide(false);
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Right);
    pop.set_offset(-GUIDE_ARROW_OVERLAP, 0);
    pop.set_can_target(false);
    pop.set_focusable(false);
    pop.set_can_focus(false);
    pop.add_css_class("wf-guide-label");
    pop.add_css_class("wf-guide-arrow");
    pop.set_child(Some(&overlay));
    {
        let (pop_w, target_w, text) = (pop.downgrade(), target.downgrade(), text.clone());
        area.set_draw_func(move |_, cr, w, h| {
            // The placement is only known once the surface exists: read it on every draw
            // and re-lay-out once if the side changed.
            if let (Some(pop), Some(target)) = (pop_w.upgrade(), target_w.upgrade()) {
                if let Some(now) = guide_arrow_tip_left(&pop, &target) {
                    if now != tip_left.get() {
                        tip_left.set(now);
                        let (pop_w, text) = (pop_w.clone(), text.clone());
                        glib::idle_add_local_once(move || {
                            if let Some(pop) = pop_w.upgrade() {
                                let (near, far) = (GUIDE_ARROW_HEAD as i32 + GUIDE_ARROW_PAD, GUIDE_ARROW_PAD);
                                text.set_margin_start(if now { near } else { far });
                                text.set_margin_end(if now { far } else { near });
                                pop.set_offset(if now { -GUIDE_ARROW_OVERLAP } else { GUIDE_ARROW_OVERLAP }, 0);
                                pop.child().map(|c| c.queue_draw());
                            }
                        });
                    }
                }
            }
            // GTK restores the input region on some layouts; keep it empty on every draw.
            if let Some(pop) = pop_w.upgrade().filter(|p| p.is_mapped() && p.root().is_some()) {
                if let Some(surface) = pop.native().and_then(|n| n.surface()) {
                    surface.set_input_region(Some(&gtk::cairo::Region::create()));
                }
            }
            let poly = guide_arrow_polygon(w as f64, h as f64, GUIDE_ARROW_HEAD, GUIDE_ARROW_SHAFT, tip_left.get());
            cr.set_source_rgb(1.0, 47.0 / 255.0, 208.0 / 255.0);
            cr.move_to(poly[0].0, poly[0].1);
            for p in &poly[1..] {
                cr.line_to(p.0, p.1);
            }
            cr.close_path();
            let _ = cr.fill();
        });
    }
    pop.connect_map(|p| {
        if p.root().is_none() {
            return;
        }
        let Some(surface) = p.native().and_then(|n| n.surface()) else { return };
        surface.set_input_region(Some(&gtk::cairo::Region::create()));
        // GTK can restore the region when the surface is laid out again: keep it empty.
        surface.connect_layout(|surface, _, _| surface.set_input_region(Some(&gtk::cairo::Region::create())));
        let weak = surface.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(surface) = weak.upgrade() {
                surface.set_input_region(Some(&gtk::cairo::Region::create()));
            }
        });
    });
    pop
}

#[cfg(feature = "guide")]
fn guide_install_css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(GUIDE_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

#[cfg(feature = "guide")]
fn guide_unpaint(state: &Rc<RefCell<AppState>>) {
    let Some(visual) = state.borrow_mut().guide_visual.take() else { return };
    guide_release_hook(visual.hook.take());
    visual.label.popdown();
    visual.label.unparent();
    if let Some(target) = visual.target.upgrade() {
        if visual.mark == GuideMark::Circle {
            target.remove_css_class("wf-guide-circle");
        }
    }
}

/// Redraws from scratch: tears down the old visual, then draws one only for a
/// known request whose live target is also showing (a closed menu is not).
#[cfg(feature = "guide")]
fn guide_paint(state: &Rc<RefCell<AppState>>) {
    guide_unpaint(state);
    let GuideRequest::Known { name, label } = state.borrow().guide_pending.clone() else { return };
    let Some(target) = guide_live_target(state) else { return };
    if !target.is_visible() {
        return;
    }
    // A missing or malformed registry mark draws nothing rather than guessing.
    let Some(mark) = guide_mark_for(&name) else { return };
    guide_install_css();
    let pop = match mark {
        GuideMark::Circle => {
            let text = gtk::Label::new(Some(&label));
            text.set_can_target(false);
            text.set_focusable(false);
            let pop = gtk::Popover::new();
            pop.set_autohide(false);
            pop.set_has_arrow(false);
            pop.set_position(gtk::PositionType::Right);
            pop.set_can_target(false);
            pop.set_focusable(false);
            pop.set_can_focus(false);
            pop.add_css_class("wf-guide-label");
            pop.set_child(Some(&text));
            target.add_css_class("wf-guide-circle");
            pop
        }
        GuideMark::Arrow => guide_arrow_popover(&target, &label),
    };
    if target.is::<gtk::ScrolledWindow>() || name == "item-menu-full-d60" || name == GUIDE_DRIVE_SPACE || name == GUIDE_CRUCIAL_BACK {
        // The item menu's scroller and content are allocated only after it maps: it resolves again
        // once laid out, so this places the popover once against the real box, never an empty one.
        if target.width() <= 0 || target.height() <= 0 {
            if name == GUIDE_DRIVE_SPACE || name == GUIDE_CRUCIAL_BACK {
                guide_retry_after_paint(state, &target);
            }
            return;
        }
        pop.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, target.width(), target.height())));
    }
    pop.set_parent(&target);
    pop.popup();
    let hook = Rc::new(RefCell::new(guide_connect_use(state, &target)));
    state.borrow_mut().guide_visual = Some(GuideVisual { target: target.downgrade(), mark, label: pop, hook });
}

/// A menu closing takes its target out of view: repaint so no visual stays.
#[cfg(feature = "guide")]
fn guide_repaint_on_close(popover: &gtk::Popover, state: &Rc<RefCell<AppState>>) {
    let weak = Rc::downgrade(state);
    popover.connect_closed(move |_| {
        if let Some(state) = weak.upgrade() {
            // Menu rows close their menu before their action runs, so the use
            // callback must outlive this repaint until the click has finished.
            let hook = state.borrow().guide_visual.as_ref().and_then(|v| v.hook.take());
            glib::idle_add_local_once(move || guide_release_hook(hook));
            guide_paint(&state);
        }
    });
}

/// The bound widget, only while it is still live in this window's tree. A
/// popover that was closed and unparented is unavailable even if something
/// still holds it (the menu buttons hold their popover, so it is not freed).
#[cfg(feature = "guide")]
#[allow(dead_code)] // first user is the ring round; the D-51 test reads it
fn guide_live_target(state: &Rc<RefCell<AppState>>) -> Option<gtk::Widget> {
    let widget = state.borrow().guide_target.as_ref()?.upgrade()?;
    let ui = ui_for_state(state)?;
    widget.is_ancestor(&ui.window).then_some(widget)
}

/// Stores the newest request, drops any old target, and looks for the new one.
#[cfg(feature = "guide")]
fn guide_apply(state: &Rc<RefCell<AppState>>, request: GuideRequest) {
    let unknown = match &request {
        GuideRequest::Unknown { name, .. } => Some(name.clone()),
        _ => None,
    };
    {
        let mut st = state.borrow_mut();
        st.guide_pending = request;
        st.guide_target = None;
    }
    guide_resolve(state);
    if let Some(name) = unknown {
        guide_write_unknown(&guide_file_path(), &name);
    }
}

#[cfg(all(feature = "guide", test))]
thread_local! {
    /// Test seam: every temp sibling a response write used, in order.
    static GUIDE_RESPONSE_TEMPS: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

#[cfg(all(feature = "guide", test))]
fn guide_note_temp(temp: &Path) {
    GUIDE_RESPONSE_TEMPS.with(|t| t.borrow_mut().push(temp.to_path_buf()));
}

#[cfg(all(feature = "guide", not(test)))]
fn guide_note_temp(_temp: &Path) {}

/// D-58: acknowledges an unknown name by atomically replacing the guide file with
/// `{"unknown":"<name>"}`: a uniquely named temp sibling in the same directory,
/// then a rename over the file. Creates no directory; a failed write or rename
/// removes the temp and reports nothing.
#[cfg(feature = "guide")]
fn guide_write_unknown(path: &Path, name: &str) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let (Some(dir), Some(base)) = (path.parent(), path.file_name()) else { return };
    let temp = dir.join(format!(
        ".{}.{}.{}.tmp",
        base.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    guide_note_temp(&temp);
    let body = serde_json::json!({ "unknown": name }).to_string();
    if fs::write(&temp, body).and_then(|_| fs::rename(&temp, path)).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

/// Watches the nearest existing ancestor of the guide file for only the next
/// path component, so a guide directory created later is still seen. Creates
/// nothing. Every arm first replaces the old monitor, then reads the file, so a
/// file written right after a directory appears is never missed.
#[cfg(feature = "guide")]
fn start_guide_watch(state: &Rc<RefCell<AppState>>) {
    let path = guide_file_path();
    if let Some(old) = state.borrow_mut().guide_monitor.take() {
        old.cancel();
    }
    state.borrow_mut().guide_watch_dir = None;
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.is_dir() {
            break;
        }
        ancestor = dir.parent();
    }
    let watched = ancestor.and_then(|dir| {
        let component = path.strip_prefix(dir).ok()?.components().next()?.as_os_str().to_os_string();
        let monitor = gio::File::for_path(dir)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .ok()?;
        Some((dir.to_path_buf(), component, monitor))
    });
    if let Some((dir, component, monitor)) = watched {
        let is_final = dir == path.parent().unwrap_or(Path::new(""));
        let weak = Rc::downgrade(state);
        let file_path = path.clone();
        monitor.connect_changed(move |_, file, other, _| {
            let named = |f: &gio::File| f.basename().is_some_and(|b| b == component);
            if !(named(file) || other.is_some_and(named)) {
                return;
            }
            let Some(state) = weak.upgrade() else { return };
            if is_final {
                let request = guide_read(&file_path);
                guide_apply(&state, request);
            } else {
                start_guide_watch(&state);
            }
        });
        let mut st = state.borrow_mut();
        st.guide_monitor = Some(monitor);
        st.guide_watch_dir = Some(dir);
    }
    let request = guide_read(&path);
    guide_apply(state, request);
}

fn main() {
    #[cfg(feature = "guide")]
    let _ = guide_markers_inert();
    if std::env::args().any(|arg| arg == "--pick-capable") {
        std::process::exit(0);
    }

    let app = adw::Application::builder()
        .application_id("com.kadaken.wayfinder")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|app| {
        app.style_manager()
            .set_color_scheme(adw::ColorScheme::PreferDark);
        install_app_css();
        export_file_manager1(app);
        // `app.open-window <folder>`: the same independent window Ctrl+N makes,
        // reachable from outside (gdbus / desktop actions) without a keystroke.
        let open_window = gio::SimpleAction::new("open-window", Some(glib::VariantTy::STRING));
        let app_for_action = app.clone();
        open_window.connect_activate(move |_, parameter| {
            let path = parameter
                .and_then(|value| value.get::<String>())
                .filter(|path| Path::new(path).is_dir());
            build_ui(&app_for_action, path, None);
        });
        app.add_action(&open_window);
    });
    app.connect_activate(|app| {
        if let Some((_, ui)) = active_window() {
            ui.window.present();
        } else {
            build_ui(
                app,
                native_core::start_path_from_args(),
                native_core::select_path_from_args(),
            );
        }
    });
    app.connect_open(|app, files, _| {
        // Chrome's download button opens the completed file through the desktop
        // handler.  When Wayfinder is already running, route that request into
        // the existing window instead of creating a second, disconnected view.
        // This both focuses the file manager and leaves the exact download
        // selected for the user.
        let initial_path = initial_path_from_files(files);
        let select_path = select_path_from_files(files);
        if let Some((state, ui)) = active_window() {
            if let Some(target) = select_path {
                state.borrow_mut().pinned_top = Some(target.clone());
                let parent = Path::new(&target)
                    .parent()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| state.borrow().cwd.clone());
                navigate(&state, parent);
                select_entry_by_path(&state, &ui, &target);
            } else if let Some(path) = initial_path {
                navigate(&state, path);
            }
            ui.window.present();
        } else {
            build_ui(app, initial_path, select_path);
            // A first launch that arrives as a GApplication `open` request
            // does not also emit `activate`.  Activate after creating the
            // window so GTK keeps the application alive just as it does for a
            // normal launcher click, without replacing the requested folder.
            app.activate();
        }
    });
    // GApplication turns positional file and folder arguments into `open`
    // requests.  Do not discard those arguments: desktop launchers use `%U`
    // to tell the file manager exactly which folder to show.  Picker flags are
    // Wayfinder-specific and GTK does not know them, so keep those local.
    let launch_args: Vec<String> = std::env::args().collect();
    app.run_with_args(&gtk_launch_args(&launch_args));
}

fn gtk_launch_args(raw_args: &[String]) -> Vec<String> {
    let program = raw_args
        .first()
        .cloned()
        .unwrap_or_else(|| "wayfinder".to_string());
    let uses_picker_flags = raw_args.iter().skip(1).any(|arg| {
        matches!(
            arg.as_str(),
            "--pick"
                | "--multiple"
                | "--directory"
                | "--out"
                | "--starting-dir"
                | "--filter"
                | "--picker-size"
                | "--picker-window"
        )
    });

    if uses_picker_flags {
        vec![program]
    } else {
        raw_args.to_vec()
    }
}

fn initial_path_from_files(files: &[gio::File]) -> Option<String> {
    files.first().and_then(|file| file.path()).and_then(|path| {
        if path.is_dir() {
            Some(path.to_string_lossy().to_string())
        } else {
            path.parent()
                .map(|parent| parent.to_string_lossy().to_string())
        }
    })
}

/// The file to highlight when opened via `%U` with a file (download managers,
/// "show in folder"). `initial_path_from_files` only yields its folder.
fn select_path_from_files(files: &[gio::File]) -> Option<String> {
    files
        .first()
        .and_then(|file| file.path())
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn is_downloads_dir(path: &str) -> bool {
    native_core::standard_dirs()
        .iter()
        .any(|(name, downloads)| name == "Downloads" && downloads == path)
}

fn install_app_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(APP_CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_ui(app: &adw::Application, initial_path: Option<String>, select_path: Option<String>) {
    let config = load_config();
    let picker = parse_picker_state();
    let initial_cwd = initial_path
        .or_else(|| {
            picker
                .as_ref()
                .and_then(|picker| picker.config.starting_dir.clone())
        })
        .or_else(native_core::start_path_from_args)
        .or_else(|| {
            native_core::standard_dirs()
                .first()
                .map(|(_, path)| path.clone())
        })
        .unwrap_or_else(|| "/".to_string());
    let state = Rc::new(RefCell::new(AppState {
        inline_rename: None,
        reveal_path: None,
        last_name_click: None,
        activation_gate: Rc::new(RefCell::new(ActivationGate::default())),
        cwd: initial_cwd.clone(),
        tabs: TabSet::new(initial_cwd),
        entries: Vec::new(),
        visible_entries: Vec::new(),
        pinned_top: select_path.clone(),
        filter_text: String::new(),
        show_hidden: config.show_hidden,
        search_in_files: config.search_in_files,
        view_mode: view_mode_from_id(&config.view_mode),
        icon_zoom: 1.0,
        preview_zoom: 1.0,
        sort_key: config.sort_key,
        sort_ascending: config.sort_ascending,
        sidebar_active_key: None,
        favorites: config.favorites,
        recents: config.recents,
        tree_expanded: HashSet::new(),
        tree_children: HashMap::new(),
        typeahead: String::new(),
        typeahead_at: Instant::now(),
        clipboard: None,
        undo_stack: Vec::new(),
        picker: picker.clone(),
        listing_generation: 0,
        listing_cancel: None,
        directory_monitor: None,
        monitor_generation: 0,
        meta_labels: HashMap::new(),
        thumbnail_queue: None,
        drives: Vec::new(),
        drive_refresh_in_flight: false,
        drive_refresh_again: false,
        drive_scan: drive_scan(),
        #[cfg(feature = "guide")]
        guide_pending: GuideRequest::Clear,
        #[cfg(feature = "guide")]
        guide_target: None,
        #[cfg(feature = "guide")]
        guide_visual: None,
        #[cfg(feature = "guide")]
        guide_monitor: None,
        #[cfg(feature = "guide")]
        guide_watch_dir: None,
    }));

    // Each path segment is its own button, so "Computer > home > zito > Downloads"
    // lets you jump straight back to any ancestor instead of pressing Up N times.
    let path_crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    path_crumbs.add_css_class("wf-address");
    path_crumbs.set_valign(gtk::Align::Center);
    let path_label = gtk::ScrolledWindow::builder()
        .child(&path_crumbs)
        .hexpand(true)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .build();
    let path_entry = gtk::Entry::builder()
        .hexpand(true)
        .activates_default(true)
        .build();
    path_entry.add_css_class("wf-address");
    let path_stack = gtk::Stack::new();
    path_stack.add_named(&path_label, Some("label"));
    path_stack.add_named(&path_entry, Some("entry"));
    path_stack.set_visible_child_name("label");
    let path_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    path_box.add_css_class("wf-address-box");
    path_box.set_hexpand(true);
    path_box.set_height_request(24);
    path_box.append(&path_stack);
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Filter here · Enter to search subfolders")
        .tooltip_text("Filter this folder by name. Press Enter to search subfolders.")
        .width_request(80)
        .height_request(24)
        .build();
    search.add_css_class("wf-search");
    let search_in_files = gtk::CheckButton::builder()
        .label("in files")
        .tooltip_text("Search inside text files when Enter searches subfolders")
        .active(state.borrow().search_in_files)
        .build();
    search_in_files.add_css_class("wf-search-scope");
    // VIRTUALIZED. gtk::ListBox realizes a widget for every row, so a folder of
    // 21,436 files built 21,436 rows and GTK then re-measured all of them on
    // every layout pass — 27 seconds of frozen window. gtk::ListView only
    // realizes the rows actually on screen (~30) and recycles them as you
    // scroll, which is what Explorer does. Row count no longer affects cost.
    let list_store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let list_selection = gtk::MultiSelection::new(Some(list_store.clone()));
    let list_factory = gtk::SignalListItemFactory::new();
    let list = gtk::ListView::builder()
        .model(&list_selection)
        .factory(&list_factory)
        .single_click_activate(false)
        .vexpand(true)
        .build();
    // An empty folder used to render as a blank void, which is indistinguishable
    // from a folder that failed to load. Say which it is, like Explorer does.
    // ListView has no placeholder slot, so it is an overlay toggled by row count.
    let list_placeholder = gtk::Label::builder()
        .label("This folder is empty")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    list_placeholder.add_css_class("wf-empty-placeholder");
    list.set_size_request(-1, 0);
    list.add_css_class("wf-list");
    // Drag a box across the list to select a run of files, like Explorer.
    // GtkListView inherits `enable-rubberband` from GtkListBase, but gtk4-rs
    // generates no typed setter for it, so set the property directly. It needs a
    // multi-selection model, which this list has.
    list.set_property("enable-rubberband", true);
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Multiple)
        .activate_on_single_click(false)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .row_spacing(8)
        .column_spacing(8)
        .valign(gtk::Align::Start)
        .vexpand(true)
        .build();
    grid.set_size_request(-1, 0);
    grid.add_css_class("wf-grid");
    let preview = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .margin_top(10)
        .margin_end(10)
        .margin_bottom(10)
        .margin_start(10)
        .build();
    preview.add_css_class("wf-preview-text");
    let preview_picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(true)
        .vexpand(true)
        .margin_top(10)
        .margin_end(10)
        .margin_bottom(10)
        .margin_start(10)
        .build();
    preview_picture.set_size_request(0, 0);
    let preview_media_picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(true)
        .vexpand(true)
        .margin_top(10)
        .margin_end(10)
        .margin_bottom(8)
        .margin_start(10)
        .build();
    preview_media_picture.set_size_request(0, 0);
    let preview_media_controls = gtk::MediaControls::new(None::<&gtk::MediaStream>);
    let preview_media_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    preview_media_box.set_hexpand(true);
    preview_media_box.set_vexpand(true);
    preview_media_box.set_size_request(0, 0);
    preview_media_box.append(&preview_media_picture);
    preview_media_box.append(&preview_media_controls);
    let preview_stack = gtk::Stack::new();
    preview_stack.add_named(&preview, Some("text"));
    preview_stack.add_named(&preview_picture, Some("image"));
    preview_stack.add_named(&preview_media_box, Some("media"));
    preview_stack.set_visible_child_name("text");
    // Spacing lives in CSS padding so the status bar's background runs the full
    // width of the window. With margins it stopped short on both sides.
    let status = gtk::Label::builder().xalign(0.0).build();
    status.add_css_class("wf-status");

    let back = icon_button("go-previous-symbolic", "Back");
    let forward = icon_button("go-next-symbolic", "Forward");
    let up = icon_button("go-up-symbolic", "Up");
    let home = icon_button("go-home-symbolic", "Home");
    let copy_path = icon_button("edit-copy-symbolic", "Copy path");
    let refresh = icon_button("view-refresh-symbolic", "Refresh");
    let hidden = gtk::ToggleButton::builder()
        .icon_name("view-reveal-symbolic")
        .tooltip_text("Show hidden files")
        .active(state.borrow().show_hidden)
        .build();
    hidden.add_css_class("wf-icon-button");
    size_toolbar_button(&hidden);
    let new_folder = icon_button("folder-new-symbolic", "New folder");
    let new_file = icon_button("document-new-symbolic", "New file");
    let undo = icon_button("edit-undo-symbolic", "Undo");
    let rename = icon_button("edit-rename-symbolic", "Rename");
    let copy = icon_button("edit-copy-symbolic", "Copy");
    let cut = icon_button("edit-cut-symbolic", "Cut");
    let paste = icon_button("edit-paste-symbolic", "Paste");
    let trash = icon_button("user-trash-symbolic", "Move to Trash");
    let terminal = icon_button("utilities-terminal-symbolic", "Terminal here");
    #[cfg(feature = "guide")]
    {
        guide_name(&terminal, "terminal-here-folder");
        guide_name(&trash, "trash-and-restore-file");
    }
    let details_view = gtk::ToggleButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Details")
        .active(state.borrow().view_mode == ViewMode::Details)
        .build();
    details_view.add_css_class("wf-icon-button");
    size_toolbar_button(&details_view);
    let compact_view = asset_toggle_button(
        "view-list-compact",
        "Compact details",
        state.borrow().view_mode == ViewMode::Compact,
    );
    let icon_view = asset_toggle_button(
        "view-grid",
        "Icons",
        state.borrow().view_mode == ViewMode::Grid,
    );
    let preview_toggle = gtk::ToggleButton::builder()
        .tooltip_text("Preview pane")
        .active(true)
        .build();
    // Icon themes differ (Adwaita has the first, Breeze the others); with none of
    // them installed the button falls back to the ▭ glyph the README names.
    match preview_toggle_icon() {
        Some(name) => preview_toggle.set_icon_name(name),
        None => preview_toggle.set_label("\u{25AD}"),
    }
    preview_toggle.add_css_class("wf-icon-button");
    size_toolbar_button(&preview_toggle);
    // No width_request: 58px was narrower than the label's natural size, so the
    // text sat clipped against the toolbar edge.
    let options = gtk::Button::builder()
        .label("Options")
        .tooltip_text("View and folder options")
        .height_request(24)
        .build();
    options.add_css_class("wf-icon-button");

    let tabbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    tabbar.add_css_class("wf-tabbar");

    let path_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    path_bar.add_css_class("wf-pathbar");
    // Padding, not margins — see wf-status. Margins left its background floating
    // 6px short of each edge.
    path_bar.append(&back);
    path_bar.append(&forward);
    path_bar.append(&up);
    path_bar.append(&home);
    let path_drag_handle = gtk::WindowHandle::new();
    path_drag_handle.set_hexpand(true);
    path_drag_handle.set_child(Some(&path_box));

    path_bar.append(&path_drag_handle);
    path_bar.append(&copy_path);
    path_bar.append(&search);
    path_bar.append(&search_in_files);

    // Explorer splits its chrome into a NAVIGATION row (back/forward/up/address
    // /search) and a COMMAND BAR (New, cut/copy/paste/rename/delete, view,
    // options). Wayfinder had all of it crammed into one row, and nine of the
    // command buttons below were built and wired up but never put on screen at
    // all, so New Folder, New File, Cut, Copy, Paste, Rename, Delete, Undo and
    // Open Terminal were unreachable from the toolbar.
    let command_bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    command_bar.add_css_class("wf-commandbar");
    command_bar.append(&new_folder);
    command_bar.append(&new_file);
    command_bar.append(&command_divider());
    command_bar.append(&cut);
    command_bar.append(&copy);
    command_bar.append(&paste);
    command_bar.append(&rename);
    command_bar.append(&trash);
    command_bar.append(&command_divider());
    command_bar.append(&undo);
    command_bar.append(&terminal);
    let command_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    command_spacer.set_hexpand(true);
    command_bar.append(&command_spacer);
    command_bar.append(&details_view);
    command_bar.append(&compact_view);
    command_bar.append(&icon_view);
    command_bar.append(&preview_toggle);
    command_bar.append(&command_divider());
    command_bar.append(&hidden);
    command_bar.append(&refresh);
    command_bar.append(&options);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .activate_on_single_click(true)
        .build();
    sidebar.set_size_request(72, 0);
    sidebar.add_css_class("navigation-sidebar");
    sidebar.add_css_class("wf-sidebar");
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar)
        .overlay_scrolling(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(72)
        .min_content_height(0)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .vexpand(true)
        .build();
    sidebar_scroll.set_size_request(72, 0);

    let files_scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .overlay_scrolling(true)
        .hexpand(true)
        .vexpand(true)
        .min_content_width(0)
        .min_content_height(0)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .build();
    files_scroll.set_size_request(-1, 0);
    let grid_scroll = gtk::ScrolledWindow::builder()
        .child(&grid)
        .hexpand(true)
        .vexpand(true)
        .min_content_width(0)
        .min_content_height(0)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .build();
    grid_scroll.set_size_request(-1, 0);
    // The empty-folder message rides above the list rather than inside it,
    // because ListView has no placeholder slot of its own.
    let list_overlay = gtk::Overlay::new();
    list_overlay.set_child(Some(&files_scroll));
    list_overlay.add_overlay(&list_placeholder);
    let files_stack = gtk::Stack::new();
    files_stack.add_css_class("wf-files-stack");
    files_stack.add_named(&list_overlay, Some("list"));
    files_stack.add_named(&grid_scroll, Some("grid"));
    files_stack.set_visible_child_name("list");
    let (column_header, columns) = details_header();
    let files_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    files_pane.add_css_class("wf-files-pane");
    files_pane.set_hexpand(true);
    files_pane.set_vexpand(true);
    files_pane.set_size_request(-1, 0);
    files_pane.append(&column_header);
    files_pane.append(&files_stack);
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&preview_stack)
        .min_content_width(0)
        .min_content_height(0)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .vexpand(true)
        .build();
    preview_scroll.set_size_request(0, 0);
    preview_scroll.set_hexpand(false);
    preview_scroll.add_css_class("wf-preview");

    let file_preview = gtk::Paned::new(gtk::Orientation::Horizontal);
    file_preview.set_hexpand(true);
    file_preview.set_vexpand(true);
    file_preview.set_size_request(-1, 0);
    // Wide handle: the hairline default is nearly impossible to grab.
    file_preview.set_wide_handle(true);
    file_preview.set_resize_start_child(true);
    file_preview.set_resize_end_child(true);
    file_preview.set_shrink_start_child(true);
    file_preview.set_shrink_end_child(true);
    file_preview.set_start_child(Some(&files_pane));
    file_preview.set_end_child(Some(&preview_scroll));
    // Initial divider set below, proportionally to the window width, so the
    // preview pane is fully visible on open instead of a ~58px sliver.

    let main = gtk::Paned::new(gtk::Orientation::Horizontal);
    main.add_css_class("wf-main");
    main.set_hexpand(true);
    main.set_vexpand(true);
    main.set_size_request(-1, 0);
    main.set_wide_handle(false);
    main.set_resize_start_child(false);
    main.set_resize_end_child(true);
    main.set_shrink_start_child(true);
    main.set_shrink_end_child(true);
    main.set_start_child(Some(&sidebar_scroll));
    main.set_end_child(Some(&file_preview));
    main.set_position(200);
    clamp_paned_position(&main, 150, 320);
    // Keep the files pane wide enough that the Name column never crushes (its
    // fixed overhead is ~388px), and the preview at least usable.
    clamp_paned_position(&file_preview, 480, 190);

    let (titlebar, minimize_button, maximize_button, close_button) = build_titlebar();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("wf-root");
    body.set_size_request(0, 0);
    body.set_hexpand(true);
    body.set_vexpand(true);
    body.append(&tabbar);
    let path_scroller = gtk::ScrolledWindow::builder()
        .child(&path_bar)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(0)
        .min_content_height(0)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .build();
    path_scroller.set_size_request(0, 0);
    body.append(&path_scroller);
    body.append(&command_bar);
    body.append(&main);

    let picker_open = if picker.is_some() {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("wf-picker-actions");
        actions.set_halign(gtk::Align::Fill);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        let cancel = gtk::Button::builder().label("Cancel").build();
        cancel.add_css_class("wf-secondary-button");

        let open = gtk::Button::builder().label("Open").build();
        open.add_css_class("wf-primary-button");

        actions.append(&spacer);
        actions.append(&cancel);
        actions.append(&open);
        body.append(&actions);

        if let Some(picker_state) = picker.clone() {
            cancel.connect_clicked(move |_| picker_write_and_exit(&picker_state, Vec::new(), 1));
        }

        Some(open)
    } else {
        None
    };

    body.append(&status);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("wf-root");
    content.set_size_request(1, 1);
    content.append(&titlebar);
    content.append(&body);

    let root_overlay = gtk::Overlay::new();
    root_overlay.set_size_request(1, 1);
    root_overlay.set_child(Some(&content));

    let picker_window = picker
        .as_ref()
        .and_then(|picker| picker.config.window)
        .unwrap_or(PickerWindow {
            width: 1180,
            height: 760,
        });

    // Give the preview pane ~42% of the files+preview area so it's fully visible
    // on open. (main sidebar is 200px; the rest is the files|preview split.)
    // Files|preview split. Sidebar is 200px; the remainder is this paned. Give the
    // preview a fixed comfortable default (300px, enough to view an image) and let
    // the file list take everything else, so the Name column has real room.
    //   Files pane fixed overhead (row): margin(20) + icon(32) + 4*gap(40) +
    //   Size(60)+Type(88)+Modified(148) = 388px, so Name = files_pane - 388.
    let available = (picker_window.width - 200).max(670);
    let files_pos = (available - 300).max(480);
    file_preview.set_position(files_pos);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Wayfinder")
        .decorated(false)
        .resizable(true)
        .default_width(picker_window.width)
        .default_height(picker_window.height)
        .content(&root_overlay)
        .build();
    window.set_size_request(1, 1);
    window.add_css_class("wayfinder-window");
    connect_window_drag(&path_drag_handle, &window);
    connect_titlebar_buttons(&window, minimize_button, maximize_button, close_button);
    add_resize_grips(&root_overlay, &window);

    let ui = Ui {
        window,
        tabbar,
        back: back.clone(),
        forward: forward.clone(),
        hidden: hidden.clone(),
        files_stack,
        column_header,
        columns,
        path_stack,
        path_crumbs,
        path_entry,
        search,
        sidebar: sidebar.clone(),
        list,
        list_store,
        list_selection,
        list_placeholder,
        list_scroll: files_scroll.clone(),
        grid,
        preview_stack,
        preview_scroll: preview_scroll.clone(),
        file_preview: file_preview.clone(),
        preview_toggle: preview_toggle.clone(),
        preview_saved_position: Rc::new(std::cell::Cell::new(0)),
        preview_picture,
        preview_media_picture,
        preview_media_controls,
        preview,
        status,
        picker_open: picker_open.clone(),
    };
    // GtkApplication keeps an activation-created window alive automatically,
    // but an initial GApplication `open` request can otherwise return before
    // GTK has taken that hold. Pair an explicit hold with the window lifecycle
    // so folder launches and regular launches behave identically.
    register_window(&state, &ui, app.hold());
    {
        let state = state.clone();
        ui.window.connect_close_request(move |_| {
            unregister_window(&state);
            glib::Propagation::Proceed
        });
    }

    rebuild_tabs(&state, &ui);
    rebuild_sidebar(&state, &ui);
    start_drive_refresh(&state);
    #[cfg(feature = "guide")]
    start_guide_watch(&state);
    {
        let state = state.clone();
        let ui = ui.clone();
        sidebar.connect_row_activated(move |_, row| activate_sidebar_row(&state, &ui, row));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.connect_pressed(move |_, n_press, _, _| {
            if n_press >= 1 {
                edit_path_action(&state, &ui);
            }
        });
        path_box.add_controller(click);
    }
    connect_file_drop(&files_scroll, state.clone(), ui.clone());
    connect_file_drop(&grid_scroll, state.clone(), ui.clone());
    connect_zoom_controller(&files_scroll, state.clone(), ui.clone(), ZoomTarget::Files);
    connect_zoom_controller(&grid_scroll, state.clone(), ui.clone(), ZoomTarget::Files);
    connect_zoom_controller(
        &preview_scroll,
        state.clone(),
        ui.clone(),
        ZoomTarget::Preview,
    );
    connect_list_factory(&list_factory, &state, &ui);
    connect_background_context(&ui.list, state.clone(), ui.clone());
    connect_grid_background_context(&grid_scroll, state.clone(), ui.clone());
    connect_actions(
        &state,
        &ui,
        back,
        forward,
        up,
        home,
        refresh,
        hidden,
        new_folder,
        new_file,
        undo,
        rename,
        copy,
        cut,
        paste,
        trash,
        terminal,
        copy_path,
        details_view,
        compact_view,
        icon_view,
        options,
    );
    connect_sort_headers(&state, &ui);
    update_sort_header_labels(&state, &ui);
    connect_shortcuts(state.clone(), ui.clone());
    connect_picker_actions(&state, &ui);

    if let Some(picker_state) = picker {
        ui.window.connect_close_request(move |_| {
            picker_write_and_exit(&picker_state, Vec::new(), 1);
        });
    }

    {
        let state = state.clone();
        search_in_files.connect_toggled(move |button| {
            let mut state_ref = state.borrow_mut();
            state_ref.search_in_files = button.is_active();
            let value = state_ref.search_in_files;
            update_config(|config| config_search_in_files(config, value));
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let entry = ui.path_entry.clone();
        entry.connect_activate(move |entry| {
            let path = entry.text().trim().to_string();
            ui.path_stack.set_visible_child_name("label");
            if !path.is_empty() {
                navigate(&state, path);
            }
        });
    }
    {
        let ui = ui.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                ui.path_stack.set_visible_child_name("label");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        ui.path_entry.add_controller(key);
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let search = ui.search.clone();
        search.connect_search_changed(move |entry| {
            state.borrow_mut().filter_text = entry.text().trim().to_string();
            repopulate_rows(&state, &ui);
            update_selection(&state, &ui);
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let search = ui.search.clone();
        search.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if !query.is_empty() {
                recursive_search_action(state.clone(), ui.clone(), query);
            }
        });
    }

    {
        let state = state.clone();
        let ui = ui.clone();
        let selection = ui.list_selection.clone();
        selection.connect_selection_changed(move |_, _, _| update_selection(&state, &ui));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let list = ui.list.clone();
        let gate = state.borrow().activation_gate.clone();
        let paths = {
            let state = state.clone();
            Rc::new(move |position: u32| {
                state.borrow().visible_entries.get(position as usize).map(|e| e.path.clone())
            }) as Rc<dyn Fn(u32) -> Option<String>>
        };
        let open = {
            let (state, ui) = (state.clone(), ui.clone());
            Rc::new(move |path: String| {
                let entry = state.borrow().visible_entries.iter().find(|e| e.path == path).cloned();
                if let Some(entry) = entry {
                    let (state, ui) = (state.clone(), ui.clone());
                    defer_after_signal(move || open_entry(&state, &ui, entry));
                }
            }) as Rc<dyn Fn(String)>
        };
        connect_gated_activate(&list, gate, paths, open);
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let grid = ui.grid.clone();
        grid.connect_selected_children_changed(move |_| update_selection(&state, &ui));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let grid = ui.grid.clone();
        grid.connect_child_activated(move |_, child| {
            if let Some(entry) = entry_for_grid_child(&state, child) {
                let (state, ui) = (state.clone(), ui.clone());
                defer_after_signal(move || open_entry(&state, &ui, entry));
            }
        });
    }

    let initial = state.borrow().cwd.clone();
    navigate(&state, initial);
    let initial_view = state.borrow().view_mode;
    set_view_mode(&state, &ui, initial_view);

    // Launched pointing at a file (download finished, "show in folder"): highlight it.
    if let Some(target) = select_path.as_deref() {
        select_entry_by_path(&state, &ui, target);
    }

    ui.window.present();
}

/// Select+focus the row for `path` in the current listing, if it is present.
fn select_entry_by_path(state: &Rc<RefCell<AppState>>, ui: &Ui, path: &str) {
    let index = state
        .borrow()
        .visible_entries
        .iter()
        .position(|entry| entry.path == path);
    if let Some(index) = index {
        select_index(state, ui, index);
    }
}

/// A thin vertical rule between groups of command-bar buttons.
fn command_divider() -> gtk::Separator {
    let sep = gtk::Separator::new(gtk::Orientation::Vertical);
    sep.add_css_class("wf-command-divider");
    sep
}

fn icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .build();
    button.add_css_class("wf-icon-button");
    size_toolbar_button(&button);
    button
}

fn asset_toggle_button(asset_name: &str, tooltip: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .tooltip_text(tooltip)
        .active(active)
        .build();
    button.add_css_class("wf-icon-button");
    size_toolbar_button(&button);

    if let Some(path) = wayfinder_ui_icon_path(asset_name) {
        let icon = gtk::Picture::for_filename(path);
        icon.set_can_shrink(true);
        icon.set_content_fit(gtk::ContentFit::Contain);
        icon.set_size_request(16, 16);
        button.set_child(Some(&icon));
    }

    button
}

fn size_toolbar_button(button: &impl IsA<gtk::Widget>) {
    button.set_width_request(26);
    button.set_height_request(24);
}

fn clamp_paned_position(paned: &gtk::Paned, min_start: i32, min_end: i32) {
    let adjusting = Rc::new(RefCell::new(false));
    let adjusting_for_notify = adjusting.clone();
    paned.connect_position_notify(move |pane| {
        if *adjusting_for_notify.borrow() {
            return;
        }

        let width = pane.width();
        if width <= 0 {
            return;
        }

        let lower = min_start.min(width.saturating_sub(1)).max(1);
        let upper = if width > min_start + min_end {
            width - min_end
        } else {
            (width * min_start / (min_start + min_end).max(1)).clamp(1, width.saturating_sub(1))
        };
        let lower = lower.min(upper);
        let position = pane.position();
        let clamped = position.clamp(lower, upper);
        if clamped != position {
            *adjusting_for_notify.borrow_mut() = true;
            pane.set_position(clamped);
            *adjusting_for_notify.borrow_mut() = false;
        }
    });
}

fn connect_window_drag<W: IsA<gtk::Widget>>(widget: &W, window: &adw::ApplicationWindow) {
    let drag = gtk::GestureClick::new();
    drag.set_button(1);
    let window = window.clone();
    drag.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }
        let Some(device) = gesture.current_event_device() else {
            return;
        };
        let Some(surface) = window.surface() else {
            return;
        };
        let Ok(toplevel) = surface.dynamic_cast::<gdk::Toplevel>() else {
            return;
        };
        toplevel.begin_move(
            &device,
            gesture.current_button() as i32,
            x,
            y,
            gesture.current_event_time(),
        );
    });
    widget.add_controller(drag);
}

fn add_resize_grips(overlay: &gtk::Overlay, window: &adw::ApplicationWindow) {
    for (edge, width, height, halign, valign, cursor) in [
        (
            gdk::SurfaceEdge::North,
            -1,
            8,
            gtk::Align::Fill,
            gtk::Align::Start,
            "n-resize",
        ),
        (
            gdk::SurfaceEdge::South,
            -1,
            8,
            gtk::Align::Fill,
            gtk::Align::End,
            "s-resize",
        ),
        (
            gdk::SurfaceEdge::West,
            8,
            -1,
            gtk::Align::Start,
            gtk::Align::Fill,
            "w-resize",
        ),
        (
            gdk::SurfaceEdge::East,
            8,
            -1,
            gtk::Align::End,
            gtk::Align::Fill,
            "e-resize",
        ),
        (
            gdk::SurfaceEdge::NorthWest,
            16,
            16,
            gtk::Align::Start,
            gtk::Align::Start,
            "nw-resize",
        ),
        (
            gdk::SurfaceEdge::NorthEast,
            16,
            16,
            gtk::Align::End,
            gtk::Align::Start,
            "ne-resize",
        ),
        (
            gdk::SurfaceEdge::SouthWest,
            16,
            16,
            gtk::Align::Start,
            gtk::Align::End,
            "sw-resize",
        ),
        (
            gdk::SurfaceEdge::SouthEast,
            16,
            16,
            gtk::Align::End,
            gtk::Align::End,
            "se-resize",
        ),
    ] {
        let grip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        grip.set_halign(halign);
        grip.set_valign(valign);
        grip.set_size_request(width, height);
        grip.set_cursor_from_name(Some(cursor));
        connect_window_resize(&grip, window, edge);
        overlay.add_overlay(&grip);
    }
}

fn connect_window_resize<W: IsA<gtk::Widget>>(
    widget: &W,
    window: &adw::ApplicationWindow,
    edge: gdk::SurfaceEdge,
) {
    let drag = gtk::GestureClick::new();
    drag.set_button(1);
    let window = window.clone();
    drag.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }
        let Some(surface) = window.surface() else {
            return;
        };
        let Ok(toplevel) = surface.dynamic_cast::<gdk::Toplevel>() else {
            return;
        };
        toplevel.begin_resize(
            edge,
            gesture.current_event_device().as_ref(),
            gesture.current_button() as i32,
            x,
            y,
            gesture.current_event_time(),
        );
    });
    widget.add_controller(drag);
}

fn build_titlebar() -> (gtk::WindowHandle, gtk::Button, gtk::Button, gtk::Button) {
    let handle = gtk::WindowHandle::new();
    handle.add_css_class("wf-titlebar");

    let layout = gtk::CenterBox::new();
    layout.add_css_class("wf-titlebar-center");
    layout.set_hexpand(true);

    let start = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    start.set_width_request(0);

    let title = gtk::Label::builder().label("Wayfinder").xalign(0.5).build();
    title.add_css_class("wf-window-title");

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let minimize = title_button("window-minimize-symbolic", "Minimize");
    let maximize = title_button("window-maximize-symbolic", "Maximize");
    let close = title_button("window-close-symbolic", "Close");
    close.add_css_class("wf-close-button");
    controls.append(&minimize);
    controls.append(&maximize);
    controls.append(&close);

    layout.set_start_widget(Some(&start));
    layout.set_center_widget(Some(&title));
    layout.set_end_widget(None::<&gtk::Widget>);

    let drag_handle = gtk::WindowHandle::new();
    drag_handle.set_hexpand(true);
    drag_handle.set_child(Some(&layout));

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outer.set_hexpand(true);
    outer.append(&drag_handle);
    outer.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    outer.append(&controls);
    handle.set_child(Some(&outer));

    (handle, minimize, maximize, close)
}

fn title_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .build();
    button.add_css_class("wf-title-button");
    button
}

fn connect_titlebar_buttons(
    window: &adw::ApplicationWindow,
    minimize: gtk::Button,
    maximize: gtk::Button,
    close: gtk::Button,
) {
    {
        let window = window.clone();
        minimize.connect_clicked(move |_| window.minimize());
    }
    {
        let window = window.clone();
        maximize.connect_clicked(move |_| {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        });
    }
    {
        let window = window.clone();
        close.connect_clicked(move |_| {
            window.close();
        });
    }
}

/// Default column widths in px. Modified fits "07/16/2026 12:00 PM" without ellipsis.
// Column widths, tuned to their widest realistic content:
//   Size    "377 MB"            ~52px
//   Type    "SHA256 file"       ~86px
//   Modified"07/13/2026 8:13 PM"~140px
// Name is flexible (hexpand) and takes whatever is left.
const COL_SIZE_W: i32 = 60;
const COL_KIND_W: i32 = 88;
const COL_MODIFIED_W: i32 = 148;
const COL_MIN_W: i32 = 44;
const COL_MAX_W: i32 = 460;
// Row leading = margin_start(10) + icon(32) + gap(10). The header uses a spacer of
// this width (minus its own margin) so the "Name" heading aligns over row names.
const ROW_ICON_SLOT: i32 = 32;
const ROW_GAP: i32 = 10;
// The draggable column separators double as the inter-column gap, so their width
// must equal the rows' box spacing (ROW_GAP) for header and rows to line up.
const COL_RESIZER_W: i32 = ROW_GAP;
// Width of the strip at the far right of the Name column, ending at the Size
// separator, where a press starts a selection box instead of dragging the file.
const BOX_SELECT_STRIP_W: f64 = 24.0;

/// Shared widths between the details header and every file row. A SizeGroup keeps
/// each column's header button and its per-row label at one width, so dragging a
/// resizer only has to resize the header.
#[derive(Clone)]
struct ColumnSizing {
    size: gtk::SizeGroup,
    kind: gtk::SizeGroup,
    modified: gtk::SizeGroup,
}

impl ColumnSizing {
    fn new() -> Self {
        let group = || gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        Self {
            size: group(),
            kind: group(),
            modified: group(),
        }
    }
}

/// A grab handle that resizes the column to its right by dragging.
fn column_resizer(target: &gtk::Button) -> gtk::DrawingArea {
    let handle = gtk::DrawingArea::new();
    handle.set_content_width(COL_RESIZER_W);
    // NOT vexpand: that propagates up and stretches the whole column header.
    handle.set_valign(gtk::Align::Fill);
    handle.add_css_class("wf-column-resizer");
    handle.set_cursor(gdk::Cursor::from_name("col-resize", None).as_ref());

    let drag = gtk::GestureDrag::new();
    let start_width = Rc::new(std::cell::Cell::new(0i32));
    {
        let target = target.clone();
        let start_width = start_width.clone();
        drag.connect_drag_begin(move |_, _, _| {
            let requested = target.size_request().0;
            start_width.set(if requested > 0 {
                requested
            } else {
                target.width().max(COL_MIN_W)
            });
        });
    }
    {
        let target = target.clone();
        let start_width = start_width.clone();
        drag.connect_drag_update(move |_, offset_x, _| {
            // Handle sits to the LEFT of its column, so dragging left widens it.
            let width = (start_width.get() as f64 - offset_x).round() as i32;
            target.set_size_request(width.clamp(COL_MIN_W, COL_MAX_W), -1);
        });
    }
    handle.add_controller(drag);
    handle
}

fn details_header() -> (gtk::Box, ColumnSizing) {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.add_css_class("wf-column-header");
    // Inset via CSS padding, NOT margins. Margins shrink the widget itself, so
    // the header's background stopped 10px short of each edge and the bar looked
    // like it was floating above a full-width file list.

    // Leading spacer so the "Name" heading lines up with the row names, which
    // begin after the icon slot + gap.
    let lead = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    lead.set_size_request(ROW_ICON_SLOT + ROW_GAP, -1);

    let name = column_button("Name", "sort-name", 0);
    name.set_hexpand(true);
    // Small max_width_chars so the header labels never dictate a floor wider than
    // the user's dragged width; the size_request below drives the real width.
    let size = column_button("Size", "sort-size", 4);
    let kind = column_button("Type", "sort-kind", 4);
    let modified = column_button("Modified", "sort-modified", 4);

    size.set_size_request(COL_SIZE_W, -1);
    kind.set_size_request(COL_KIND_W, -1);
    modified.set_size_request(COL_MODIFIED_W, -1);

    let sizing = ColumnSizing::new();
    sizing.size.add_widget(&size);
    sizing.kind.add_widget(&kind);
    sizing.modified.add_widget(&modified);

    header.append(&lead);
    header.append(&name);
    header.append(&column_resizer(&size));
    header.append(&size);
    header.append(&column_resizer(&kind));
    header.append(&kind);
    header.append(&column_resizer(&modified));
    header.append(&modified);
    (header, sizing)
}

fn column_button(text: &str, name: &str, width_chars: i32) -> gtk::Button {
    // halign Fill makes the label span its button so xalign has something to
    // align within — without it the text sits centred and drifts off the column.
    //
    // hexpand must stay FALSE here. Setting it propagates up and makes every
    // heading button expand, so Size/Type/Modified each stole a share of the
    // slack that belongs to Name alone, and the whole header packed ~130px left
    // of the values beneath it. Only the Name button opts into hexpand, and it
    // does so in details_header().
    let label = gtk::Label::builder()
        .label(text)
        .xalign(if name == "sort-size" { 1.0 } else { 0.0 })
        .yalign(0.5)
        .hexpand(false)
        .halign(gtk::Align::Fill)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    if width_chars > 0 {
        label.set_max_width_chars(width_chars);
    }
    let button = gtk::Button::builder().child(&label).build();
    button.set_widget_name(name);
    button.add_css_class("flat");
    button
}

fn rebuild_tabs(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    while let Some(child) = ui.tabbar.first_child() {
        ui.tabbar.remove(&child);
    }

    let (tabs, active_tab) = {
        let state = state.borrow();
        (
            state.tabs.tabs.iter().map(|t| t.path.clone()).collect::<Vec<_>>(),
            state.tabs.active,
        )
    };

    for (index, path) in tabs.iter().enumerate() {
        let label = tab_label(path);
        let tab = gtk::Button::builder()
            .label(&label)
            .tooltip_text(path)
            .build();
        tab.add_css_class("wf-tab");
        if index == active_tab {
            tab.add_css_class("wf-tab-active");
        }
        let state_for_tab = state.clone();
        tab.connect_clicked(move |_| switch_tab_action(state_for_tab.clone(), index));
        ui.tabbar.append(&tab);
    }

    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("New tab")
        .build();
    add.add_css_class("wf-tab-add");
    let state_for_add = state.clone();
    let ui_for_add = ui.clone();
    add.connect_clicked(move |_| new_tab_action(state_for_add.clone(), ui_for_add.clone()));
    ui.tabbar.append(&add);
    ui.tabbar.append(&tab_drag_handle(&ui.window));
}

fn tab_drag_handle(window: &adw::ApplicationWindow) -> gtk::WindowHandle {
    let filler = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    filler.set_hexpand(true);
    filler.set_height_request(28);

    let handle = gtk::WindowHandle::new();
    handle.set_hexpand(true);
    handle.set_child(Some(&filler));
    connect_window_drag(&handle, window);
    handle
}

fn tab_label(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if path == "/" {
                "/".to_string()
            } else {
                path.to_string()
            }
        })
}

fn switch_tab_action(state: Rc<RefCell<AppState>>, index: usize) {
    let path = match state.borrow_mut().tabs.switch(index) {
        Some(path) => path,
        None => return,
    };
    // Sensitivity follows the newly active tab now, not when its listing ends.
    if let Some(ui) = ui_for_state(&state) {
        update_nav_buttons(&state, &ui);
    }
    navigate_without_history(&state, path);
}

/// Open `path` in a new tab and switch to it, the way a middle-click or
/// "Open in New Tab" is expected to behave.
fn open_path_in_new_tab(state: &Rc<RefCell<AppState>>, ui: &Ui, path: String) {
    state.borrow_mut().tabs.open(path.clone());
    rebuild_tabs(state, ui);
    update_nav_buttons(state, ui);
    navigate_without_history(state, path);
}

fn new_tab_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let path = state.borrow().cwd.clone();
    state.borrow_mut().tabs.open(path.clone());
    rebuild_tabs(&state, &ui);
    update_nav_buttons(&state, &ui);
    navigate_without_history(&state, path);
}

fn close_tab_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let path = match state.borrow_mut().tabs.close_active() {
        Some(path) => path,
        None => {
            ui.status.set_label("Keep at least one tab open");
            return;
        }
    };
    rebuild_tabs(&state, &ui);
    update_nav_buttons(&state, &ui);
    navigate_without_history(&state, path);
}

fn edit_path_action(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let cwd = state.borrow().cwd.clone();
    ui.path_entry.set_text(&cwd);
    ui.path_stack.set_visible_child_name("entry");
    ui.path_entry.grab_focus();
    ui.path_entry.select_region(0, -1);
}

fn rebuild_sidebar(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    // D-76: the old Devices heading goes away below, so only this request's own visual comes off
    // first; another tutorial painted elsewhere is never touched.
    #[cfg(feature = "guide")]
    if guide_waits_for_drive_space(state) {
        guide_unpaint(state);
    }
    while let Some(child) = ui.sidebar.first_child() {
        ui.sidebar.remove(&child);
    }

    let (cwd, sidebar_active_key) = {
        let state = state.borrow();
        (state.cwd.clone(), state.sidebar_active_key.clone())
    };
    let mut active_target: Option<String> = None;
    let favorites = state.borrow().favorites.clone();
    if !favorites.is_empty() {
        append_sidebar_heading(&ui.sidebar, "Favorites");
        for favorite in favorites {
            let row = sidebar_row(&favorite.name, "", "emblem-favorite-symbolic");
            row.set_tooltip_text(Some(&favorite.path));
            mark_sidebar_active(
                &row,
                &cwd,
                &favorite.name,
                &favorite.path,
                sidebar_active_key.as_deref(),
                &mut active_target,
            );
            ui.sidebar.append(&row);
        }
    }

    append_sidebar_heading(&ui.sidebar, "Places");
    for (name, path) in native_core::standard_dirs() {
        let row = sidebar_row(&name, "", place_icon(&name));
        row.set_tooltip_text(Some(&path));
        mark_sidebar_active(
            &row,
            &cwd,
            &name,
            &path,
            sidebar_active_key.as_deref(),
            &mut active_target,
        );
        ui.sidebar.append(&row);
    }

    let drives = state.borrow().drives.clone();
    if !drives.is_empty() {
        #[cfg(feature = "guide")]
        guide_name(&append_sidebar_heading(&ui.sidebar, "Devices"), GUIDE_DRIVE_SPACE);
        #[cfg(not(feature = "guide"))]
        append_sidebar_heading(&ui.sidebar, "Devices");
        for drive in drives {
            let view = drive.space_view();
            let row = sidebar_row_with_bar(&drive.name, &view.words, drive_icon(&drive), view.fraction);
            if !drive.mountpoint.is_empty() {
                mark_sidebar_active(
                    &row,
                    &cwd,
                    &drive.name,
                    &drive.mountpoint,
                    sidebar_active_key.as_deref(),
                    &mut active_target,
                );
            }
            row.set_tooltip_text(Some(if drive.mountpoint.is_empty() {
                &drive.path
            } else {
                &drive.mountpoint
            }));
            connect_drive_context(&row, state.clone(), ui.clone(), drive);
            ui.sidebar.append(&row);
        }
    }

    let folders = folder_tree_roots();
    if !folders.is_empty() {
        append_sidebar_heading(&ui.sidebar, "Folders");
        for (name, path) in folders {
            let expanded = state.borrow().tree_expanded.contains(&path);
            let label = format!("{}  {name}", if expanded { "▾" } else { "▸" });
            let row = sidebar_row(&label, "", "asset:folders");
            row.set_tooltip_text(Some(&path));
            mark_sidebar_active(
                &row,
                &cwd,
                &label,
                &path,
                sidebar_active_key.as_deref(),
                &mut active_target,
            );
            ui.sidebar.append(&row);
            if expanded {
                let children = ensure_tree_children(state, &path);
                for (child_name, child_path, has_children) in children {
                    let child_label =
                        format!("  {}  {child_name}", if has_children { "▸" } else { " " });
                    let child_row = sidebar_row(&child_label, "", "asset:folders");
                    child_row.set_tooltip_text(Some(&child_path));
                    mark_sidebar_active(
                        &child_row,
                        &cwd,
                        &child_label,
                        &child_path,
                        sidebar_active_key.as_deref(),
                        &mut active_target,
                    );
                    ui.sidebar.append(&child_row);
                }
            }
        }
    }

    let recents = state.borrow().recents.clone();
    if !recents.is_empty() {
        append_sidebar_heading(&ui.sidebar, "Recent");
        for recent in recents {
            let row = sidebar_row(&recent.name, "", "asset:recent");
            row.set_tooltip_text(Some(&recent.path));
            mark_sidebar_active(
                &row,
                &cwd,
                &recent.name,
                &recent.path,
                sidebar_active_key.as_deref(),
                &mut active_target,
            );
            ui.sidebar.append(&row);
        }
    }
    #[cfg(feature = "guide")]
    guide_resolve_drive_space_later(state);
}

fn activate_sidebar_row(state: &Rc<RefCell<AppState>>, ui: &Ui, row: &gtk::ListBoxRow) {
    let Some(path) = row.tooltip_text().map(|s| s.to_string()) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Some(label) = sidebar_row_label(row) else {
        return;
    };
    state.borrow_mut().sidebar_active_key = Some(sidebar_key(&label, &path));

    if label.trim_start().starts_with('▸') || label.trim_start().starts_with('▾') {
        toggle_tree_path(state, ui, &path);
        return;
    }

    let cached = state
        .borrow()
        .drives
        .iter()
        .find(|drive| drive.mountpoint == path || drive.path == path)
        .cloned();
    if let Some(drive) = cached {
        open_drive_action(state.clone(), ui.clone(), drive);
    } else {
        navigate(state, path);
    }
}

fn sidebar_key(label: &str, path: &str) -> String {
    format!("{label}\n{path}")
}

fn sidebar_row_label(row: &gtk::ListBoxRow) -> Option<String> {
    row.child()
        .and_then(|line| line.last_child())
        .and_then(|text| text.first_child())
        .and_then(|name| name.downcast::<gtk::Label>().ok())
        .map(|label| label.text().to_string())
}

fn folder_tree_roots() -> Vec<(String, String)> {
    std::env::var("HOME")
        .ok()
        .filter(|home| PathBuf::from(home).is_dir())
        .map(|home| vec![("Home".to_string(), home)])
        .unwrap_or_default()
}

fn ensure_tree_children(state: &Rc<RefCell<AppState>>, path: &str) -> Vec<(String, String, bool)> {
    if let Some(children) = state.borrow().tree_children.get(path).cloned() {
        return children;
    }
    let show_hidden = state.borrow().show_hidden;
    let children = list_subdirs(path, show_hidden);
    state
        .borrow_mut()
        .tree_children
        .insert(path.to_string(), children.clone());
    children
}

fn toggle_tree_path(state: &Rc<RefCell<AppState>>, ui: &Ui, path: &str) {
    {
        let mut state_ref = state.borrow_mut();
        if state_ref.tree_expanded.contains(path) {
            state_ref.tree_expanded.remove(path);
        } else {
            state_ref.tree_expanded.insert(path.to_string());
        }
    }
    navigate(state, path.to_string());
    rebuild_sidebar(state, ui);
}

fn list_subdirs(path: &str, show_hidden: bool) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    let Ok(read_dir) = fs::read_dir(path) else {
        return out;
    };
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let child_path = entry.path().to_string_lossy().to_string();
        let has_children = fs::read_dir(&child_path)
            .map(|children| {
                children.flatten().any(|child| {
                    child.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
                        && (show_hidden || !child.file_name().to_string_lossy().starts_with('.'))
                })
            })
            .unwrap_or(false);
        out.push((name, child_path, has_children));
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

fn mark_sidebar_active(
    row: &gtk::ListBoxRow,
    cwd: &str,
    label: &str,
    path: &str,
    sidebar_active_key: Option<&str>,
    active_target: &mut Option<String>,
) {
    let key = sidebar_key(label, path);
    let should_activate = sidebar_active_key
        .map(|active_key| active_key == key)
        .unwrap_or_else(|| cwd == path && active_target.is_none());
    if should_activate {
        row.add_css_class("wf-sidebar-active");
        *active_target = Some(key);
    }
}

fn update_sidebar_active_rows(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let (cwd, sidebar_active_key) = {
        let state = state.borrow();
        (state.cwd.clone(), state.sidebar_active_key.clone())
    };
    let mut active_set = false;
    let mut child = ui.sidebar.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        let Ok(row) = widget.downcast::<gtk::ListBoxRow>() else {
            continue;
        };
        row.remove_css_class("wf-sidebar-active");
        let Some(path) = row.tooltip_text().map(|path| path.to_string()) else {
            continue;
        };
        let Some(label) = sidebar_row_label(&row) else {
            continue;
        };
        let key = sidebar_key(&label, &path);
        let should_activate = sidebar_active_key
            .as_deref()
            .map(|active_key| active_key == key)
            .unwrap_or_else(|| !active_set && path == cwd);
        if should_activate {
            row.add_css_class("wf-sidebar-active");
            active_set = true;
        }
    }
}

fn append_sidebar_heading(sidebar: &gtk::ListBox, text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .build();
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .margin_top(9)
        .margin_end(10)
        .margin_bottom(3)
        .margin_start(10)
        .build();
    label.add_css_class("wf-sidebar-heading");
    row.set_child(Some(&label));
    sidebar.append(&row);
    row
}

fn sidebar_row(name: &str, detail: &str, icon_name: &str) -> gtk::ListBoxRow {
    sidebar_row_with_bar(name, detail, icon_name, None)
}

/// The one sidebar row layout; a drive row adds a fill bar below its words.
fn sidebar_row_with_bar(
    name: &str,
    detail: &str,
    icon_name: &str,
    fraction: Option<f64>,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder().activatable(true).build();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.set_margin_top(4);
    line.set_margin_end(10);
    line.set_margin_bottom(4);
    line.set_margin_start(10);
    let icon = sidebar_icon(icon_name);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let name = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    name.set_max_width_chars(12);
    name.add_css_class("wf-sidebar-title");
    let detail = gtk::Label::builder()
        .label(detail)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    detail.set_max_width_chars(14);
    detail.add_css_class("wf-sidebar-detail");
    text.append(&name);
    if !detail.label().is_empty() {
        text.append(&detail);
    }
    if let Some(fraction) = fraction {
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("wf-space-bar");
        bar.set_fraction(fraction.clamp(0.0, 1.0));
        bar.set_can_target(false);
        bar.set_can_focus(false);
        text.append(&bar);
    }
    line.append(&icon);
    line.append(&text);
    row.set_child(Some(&line));
    row
}

fn place_icon(name: &str) -> &'static str {
    match name {
        "Home" => "asset:home",
        "Desktop" => "asset:desktop",
        "Documents" => "asset:documents",
        "Downloads" => "asset:downloads",
        "Pictures" => "asset:pictures",
        "Music" => "asset:music",
        "Videos" => "asset:video",
        "Root" => "asset:folders",
        "Trash" => "asset:trash",
        _ => "asset:folders",
    }
}

fn drive_icon(drive: &Drive) -> &'static str {
    if drive.kind == "network" {
        "asset:network"
    } else if drive.kind == "gdrive" {
        "asset:gdrive"
    } else {
        "asset:storage"
    }
}

fn sidebar_icon(icon_name: &str) -> gtk::Box {
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.set_size_request(18, 18);
    slot.set_halign(gtk::Align::Center);
    slot.set_valign(gtk::Align::Center);

    if let Some(asset_name) = icon_name.strip_prefix("asset:") {
        if let Some(path) = old_wayfinder_asset_icon_path(asset_name) {
            let icon = gtk::Picture::for_filename(path);
            icon.set_can_shrink(true);
            icon.set_content_fit(gtk::ContentFit::Contain);
            icon.set_size_request(18, 18);
            icon.set_halign(gtk::Align::Center);
            icon.set_valign(gtk::Align::Center);
            slot.append(&icon);
            return slot;
        }
    }

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    icon.set_size_request(18, 18);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    slot.append(&icon);
    slot
}

fn old_wayfinder_asset_icon_path(name: &str) -> Option<PathBuf> {
    let filename = format!("{name}.png");
    let mut candidates = Vec::new();

    if let Some(project_dir) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(project_dir.join("static/icons/sidebar").join(&filename));
        candidates.push(project_dir.join("static/icons").join(&filename));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(&home)
                .join(".local/share/wayfinder/icons/sidebar")
                .join(&filename),
        );
        candidates.push(
            PathBuf::from(home)
                .join(".local/share/wayfinder/icons")
                .join(&filename),
        );
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("static/icons/sidebar").join(&filename));
        candidates.push(cwd.join("static/icons").join(&filename));
    }

    candidates.into_iter().find(|path| path.exists())
}

fn wayfinder_ui_icon_path(name: &str) -> Option<PathBuf> {
    let filename = format!("{name}.png");
    let mut candidates = Vec::new();

    if let Some(project_dir) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(project_dir.join("static/icons/ui").join(&filename));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".local/share/wayfinder/icons/ui")
                .join(&filename),
        );
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("static/icons/ui").join(&filename));
    }

    candidates.into_iter().find(|path| path.exists())
}

#[allow(clippy::too_many_arguments)]
fn connect_actions(
    state: &Rc<RefCell<AppState>>,
    ui: &Ui,
    back: gtk::Button,
    forward: gtk::Button,
    up: gtk::Button,
    home: gtk::Button,
    refresh: gtk::Button,
    hidden: gtk::ToggleButton,
    new_folder: gtk::Button,
    new_file: gtk::Button,
    undo: gtk::Button,
    rename: gtk::Button,
    copy: gtk::Button,
    cut: gtk::Button,
    paste: gtk::Button,
    trash: gtk::Button,
    terminal: gtk::Button,
    copy_path: gtk::Button,
    details_view: gtk::ToggleButton,
    compact_view: gtk::ToggleButton,
    icon_view: gtk::ToggleButton,
    options: gtk::Button,
) {
    {
        let state = state.clone();
        back.connect_clicked(move |_| {
            history_step(&state, false);
        });
    }
    {
        let state = state.clone();
        forward.connect_clicked(move |_| {
            history_step(&state, true);
        });
    }
    {
        let state = state.clone();
        up.connect_clicked(move |_| {
            let parent = native_core::parent_dir(&state.borrow().cwd);
            navigate(&state, parent);
        });
    }
    {
        let state = state.clone();
        home.connect_clicked(move |_| {
            if let Some((_, path)) = native_core::standard_dirs().into_iter().next() {
                navigate(&state, path);
            }
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        refresh.connect_clicked(move |_| {
            let cwd = state.borrow().cwd.clone();
            rebuild_sidebar(&state, &ui);
            navigate_without_history(&state, cwd);
        });
    }
    {
        let state = state.clone();
        hidden.connect_toggled(move |button| set_show_hidden(&state, button.is_active()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        ui.preview_toggle
            .clone()
            .connect_toggled(move |button| set_preview_visible(&state, &ui, button.is_active()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        new_folder.connect_clicked(move |_| create_folder_action(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        new_file.connect_clicked(move |_| create_file_action(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        undo.connect_clicked(move |_| undo_action(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        rename.connect_clicked(move |_| rename_action(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        copy.connect_clicked(move |_| copy_selection(&state, &ui));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        cut.connect_clicked(move |_| cut_selection(&state, &ui));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        paste.connect_clicked(move |_| paste_clipboard(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        trash.connect_clicked(move |_| trash_selection(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        terminal.connect_clicked(move |_| terminal_here(state.clone(), ui.clone()));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        copy_path.connect_clicked(move |_| copy_paths_to_system_clipboard(&state, &ui));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let compact_view = compact_view.clone();
        let icon_view = icon_view.clone();
        details_view.connect_clicked(move |button| {
            button.set_active(true);
            compact_view.set_active(false);
            icon_view.set_active(false);
            set_view_mode(&state, &ui, ViewMode::Details);
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let details_view = details_view.clone();
        let icon_view = icon_view.clone();
        compact_view.connect_clicked(move |button| {
            button.set_active(true);
            details_view.set_active(false);
            icon_view.set_active(false);
            set_view_mode(&state, &ui, ViewMode::Compact);
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let details_view = details_view.clone();
        let compact_view = compact_view.clone();
        icon_view.connect_clicked(move |button| {
            button.set_active(true);
            details_view.set_active(false);
            compact_view.set_active(false);
            set_view_mode(&state, &ui, ViewMode::Grid);
        });
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let options_for_popover = options.clone();
        options.connect_clicked(move |_| {
            show_options_popover(&options_for_popover, state.clone(), ui.clone());
        });
    }
}

/// The first right-pane icon the current theme actually has (Adwaita and Breeze
/// name it differently), or None so the button falls back to the glyph.
fn preview_toggle_icon() -> Option<&'static str> {
    let display = gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    [
        "sidebar-show-right-symbolic",
        "sidebar-expand-right-symbolic",
        "view-preview-symbolic",
        "fileview-preview-symbolic",
    ]
    .into_iter()
    .find(|name| theme.has_icon(name))
}

/// The one route that shows or hides the whole preview pane. Hiding releases
/// every live preview resource and remembers the divider; showing restores the
/// divider and renders the current selection through the normal path. Setting
/// the current value is a no-op.
fn set_preview_visible(state: &Rc<RefCell<AppState>>, ui: &Ui, visible: bool) {
    if ui.preview_scroll.is_visible() == visible {
        if ui.preview_toggle.is_active() != visible {
            ui.preview_toggle.set_active(visible);
        }
        return;
    }
    if visible {
        ui.preview_scroll.set_visible(true);
        let saved = ui.preview_saved_position.get();
        if saved > 0 {
            ui.file_preview.set_position(saved);
        }
    } else {
        ui.preview_saved_position.set(ui.file_preview.position());
        // Clears media stream and paintable while the guards still pass.
        set_preview_text(ui, "Select a file");
        ui.preview_picture.set_file(None::<&gio::File>);
        ui.preview_scroll.set_visible(false);
    }
    if ui.preview_toggle.is_active() != visible {
        ui.preview_toggle.set_active(visible);
    }
    if visible {
        update_selection(state, ui);
    }
}

/// The one Show Hidden path: only the flag and its config field change, then the
/// current folder reloads without a history visit. Setting the current value is
/// a no-op.
fn set_show_hidden(state: &Rc<RefCell<AppState>>, value: bool) {
    if state.borrow().show_hidden == value {
        return;
    }
    state.borrow_mut().show_hidden = value;
    update_config(|config| config_show_hidden(config, value));
    if let Some(ui) = ui_for_state(state) {
        if ui.hidden.is_active() != value {
            ui.hidden.set_active(value);
        }
    }
    let cwd = state.borrow().cwd.clone();
    navigate_without_history(state, cwd);
}

fn set_view_mode(state: &Rc<RefCell<AppState>>, ui: &Ui, mode: ViewMode) {
    {
        let mut state_ref = state.borrow_mut();
        state_ref.view_mode = mode;
        update_config(|config| config_view_mode(config, mode));
    }
    match mode {
        ViewMode::Details => {
            ui.grid.unselect_all();
            ui.list.remove_css_class("wf-list-compact");
            ui.column_header.set_visible(true);
            ui.files_stack.set_visible_child_name("list");
        }
        ViewMode::Compact => {
            ui.grid.unselect_all();
            ui.list.add_css_class("wf-list-compact");
            ui.column_header.set_visible(true);
            ui.files_stack.set_visible_child_name("list");
        }
        ViewMode::Grid => {
            ui.list_selection.unselect_all();
            ui.list.remove_css_class("wf-list-compact");
            ui.column_header.set_visible(false);
            ui.files_stack.set_visible_child_name("grid");
        }
    }
    // Construct only the active presentation. Previously every refresh built
    // both the list and grid, including image thumbnails hidden behind Details.
    repopulate_rows(state, ui);
    update_selection(state, ui);
}

fn connect_sort_headers(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let Some(mut child) = ui.column_header.first_child() else {
        return;
    };
    loop {
        if let Ok(button) = child.clone().downcast::<gtk::Button>() {
            let key = match button.widget_name().as_str() {
                "sort-size" => Some(SortKey::Size),
                "sort-kind" => Some(SortKey::Kind),
                "sort-modified" => Some(SortKey::Modified),
                "sort-name" => Some(SortKey::Name),
                _ => None,
            };
            if let Some(key) = key {
                let state = state.clone();
                let ui = ui.clone();
                button.connect_clicked(move |_| {
                    {
                        let mut state_ref = state.borrow_mut();
                        if state_ref.sort_key == key {
                            state_ref.sort_ascending = !state_ref.sort_ascending;
                        } else {
                            state_ref.sort_key = key;
                            // Explorer-style date sorting starts with the newest
                            // item at the top; a second click reverses it.
                            state_ref.sort_ascending = key != SortKey::Modified;
                        }
                        let (key, ascending) = (state_ref.sort_key, state_ref.sort_ascending);
                        update_config(|config| config_sort(config, key, ascending));
                    }
                    update_sort_header_labels(&state, &ui);
                    repopulate_rows(&state, &ui);
                    update_selection(&state, &ui);
                });
            }
        }
        let Some(next) = child.next_sibling() else {
            break;
        };
        child = next;
    }
}

/// Show which column is active and its direction, so sorting is discoverable.
fn update_sort_header_labels(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let (active_key, ascending) = {
        let state_ref = state.borrow();
        (state_ref.sort_key, state_ref.sort_ascending)
    };
    let mut child = ui.column_header.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        let Ok(button) = widget.downcast::<gtk::Button>() else {
            continue;
        };
        let (key, title) = match button.widget_name().as_str() {
            "sort-name" => (SortKey::Name, "Name"),
            "sort-size" => (SortKey::Size, "Size"),
            "sort-kind" => (SortKey::Kind, "Type"),
            "sort-modified" => (SortKey::Modified, "Modified"),
            _ => continue,
        };
        let direction = if key == active_key {
            if ascending {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        };
        if let Some(label) = button.child().and_downcast::<gtk::Label>() {
            label.set_label(&format!("{title}{direction}"));
        }
    }
}

fn show_options_popover(parent: &gtk::Button, state: Rc<RefCell<AppState>>, ui: Ui) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_parent(parent);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(4);
    menu.set_margin_end(4);
    menu.set_margin_bottom(4);
    menu.set_margin_start(4);

    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("folder-new-symbolic", "New Folder", "Ctrl+Shift+N", &popover, move || {
            create_folder_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("document-new-symbolic", "New File", "", &popover, move || {
            create_file_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("view-refresh-symbolic", "Refresh", "F5", &popover, move || {
            let cwd = state.borrow().cwd.clone();
            rebuild_sidebar(&state, &ui);
            navigate_without_history(&state, cwd);
        }));
    }
    {
        // Checked mirror of the toolbar toggle; clicking flips that toggle, whose
        // handler is the only show-hidden path.
        let checked = state.borrow().show_hidden;
        let ui = ui.clone();
        let row = context_item(
            if checked { "object-select-symbolic" } else { "" },
            "Show Hidden Files",
            "",
            &popover,
            move || ui.hidden.set_active(!ui.hidden.is_active()),
        );
        row.set_widget_name("wf-show-hidden-option");
        if checked {
            row.add_css_class("wf-checked");
        }
        menu.append(&row);
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("", "Invert Selection", "", &popover, move || {
            invert_selection(&state, &ui)
        }));
    }
    {
        let row_state = state.clone();
        let ui = ui.clone();
        let row = context_item("", "Clear Recents", "", &popover, move || clear_recents(&ui));
        row.set_sensitive(!recents_are_empty(&row_state));
        menu.append(&row);
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("document-edit-symbolic", "Rename", "F2", &popover, move || {
            rename_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-copy-symbolic", "Copy", "Ctrl+C", &popover, move || {
            copy_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-cut-symbolic", "Cut", "Ctrl+X", &popover, move || {
            cut_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-paste-symbolic", "Paste", "Ctrl+V", &popover, move || {
            paste_clipboard(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("user-trash-symbolic", "Move to Trash", "Delete", &popover, move || {
            trash_selection(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Undo", &popover, move || {
            undo_action(state.clone(), ui.clone())
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("utilities-terminal-symbolic", "Open Terminal Here", "", &popover, move || {
            terminal_here(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("user-trash-full-symbolic", "Empty Trash", "", &popover, move || {
            empty_trash_action(state.clone(), ui.clone())
        }));
    }

    popover.set_child(Some(&menu));
    popover.popup();
}

/// Ctrl+N alone. Ctrl+Shift+N stays New Folder and Ctrl+T stays New Tab.
fn is_new_window_shortcut(ctrl: bool, shift: bool, alt: bool, key: gdk::Key) -> bool {
    ctrl && !shift && !alt && (key == gdk::Key::n || key == gdk::Key::N)
}

/// Opens `path` in its own independent window of this same application.
fn open_new_window(ui: &Ui, path: String) {
    if let Some(app) = ui
        .window
        .application()
        .and_then(|app| app.downcast::<adw::Application>().ok())
    {
        build_ui(&app, Some(path), None);
    }
}

fn connect_shortcuts(state: Rc<RefCell<AppState>>, ui: Ui) {
    let key = gtk::EventControllerKey::new();
    let window = ui.window.clone();
    let state_for_keys = state.clone();
    let ui_for_keys = ui.clone();
    key.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);

        if ctrl && shift && (key == gdk::Key::C || key == gdk::Key::c) {
            copy_paths_to_system_clipboard(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        if ctrl && shift && (key == gdk::Key::N || key == gdk::Key::n) {
            create_folder_action(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if is_new_window_shortcut(ctrl, shift, alt, key) {
            open_new_window(&ui_for_keys, state_for_keys.borrow().cwd.clone());
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::D || key == gdk::Key::d) {
            trash_selection(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if (ctrl && (key == gdk::Key::F || key == gdk::Key::f)) || key == gdk::Key::F3 {
            ui_for_keys.search.grab_focus();
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F4 || (alt && (key == gdk::Key::D || key == gdk::Key::d)) {
            edit_path_action(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F11 {
            let window = &ui_for_keys.window;
            if window.is_fullscreen() {
                window.unfullscreen();
            } else {
                window.fullscreen();
            }
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Return {
            let selection = selected_entries(&state_for_keys, &ui_for_keys);
            let path = selection
                .first()
                .map(|entry| entry.path.clone())
                .unwrap_or_else(|| state_for_keys.borrow().cwd.clone());
            show_properties_for_path(ui_for_keys.clone(), path);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Up {
            let parent = native_core::parent_dir(&state_for_keys.borrow().cwd);
            navigate(&state_for_keys, parent);
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::A || key == gdk::Key::a) {
            if state_for_keys.borrow().view_mode == ViewMode::Grid {
                ui_for_keys.grid.select_all();
            } else {
                ui_for_keys.list_selection.select_all();
            }
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::C || key == gdk::Key::c) {
            copy_selection(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::X || key == gdk::Key::x) {
            cut_selection(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::V || key == gdk::Key::v) {
            paste_clipboard(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::Z || key == gdk::Key::z) {
            undo_action(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::T || key == gdk::Key::t) {
            new_tab_action(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::W || key == gdk::Key::w) {
            close_tab_action(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if ctrl && (key == gdk::Key::L || key == gdk::Key::l) {
            edit_path_action(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Left {
            history_step(&state_for_keys, false);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Right {
            history_step(&state_for_keys, true);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F2 {
            rename_action(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Delete && shift {
            permanent_delete_selection(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Delete {
            trash_selection(state_for_keys.clone(), ui_for_keys.clone());
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Return {
            open_selected(&state_for_keys, &ui_for_keys);
            return glib::Propagation::Stop;
        }
        // Only handle plain arrows. With Shift held, let the event through so the
        // list's own selection model extends the range — intercepting it here
        // forced a single selection and made Shift+Arrow impossible.
        if !alt && !shift && key == gdk::Key::Down {
            move_selection(&state_for_keys, &ui_for_keys, 1);
            return glib::Propagation::Stop;
        }
        if !alt && !shift && key == gdk::Key::Up {
            move_selection(&state_for_keys, &ui_for_keys, -1);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::BackSpace {
            history_step(&state_for_keys, false);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F5 {
            let cwd = state_for_keys.borrow().cwd.clone();
            navigate(&state_for_keys, cwd);
            return glib::Propagation::Stop;
        }
        if !ctrl && !alt {
            if let Some(ch) = key.to_unicode().filter(|ch| !ch.is_control()) {
                typeahead_select(&state_for_keys, &ui_for_keys, ch);
                return glib::Propagation::Stop;
            }
        }

        glib::Propagation::Proceed
    });
    window.add_controller(key);
}

fn navigate(state: &Rc<RefCell<AppState>>, path: String) {
    navigate_to(state, path, NavMode::Record);
}

fn navigate_without_history(state: &Rc<RefCell<AppState>>, path: String) {
    navigate_to(state, path, NavMode::Keep);
}

/// Back or forward in the ACTIVE tab; its cursor moves only when the listing
/// succeeds.
fn history_step(state: &Rc<RefCell<AppState>>, forward: bool) {
    let target = {
        let state = state.borrow();
        if forward { state.tabs.forward_target() } else { state.tabs.back_target() }
    };
    if let Some((index, path)) = target {
        navigate_to(state, path, NavMode::Cursor(index));
    }
}

fn navigate_to(state: &Rc<RefCell<AppState>>, path: String, mode: NavMode) {
    navigate_to_with_message(state, path, mode, None);
}

/// `done_message` is a one-use status (a finished job's result). It is shown
/// only after THIS request's listing succeeds and settles, so the refresh the
/// job triggered cannot overwrite it. A listing error, a stale or cancelled
/// request, or any newer navigation never shows it.
fn navigate_to_with_message(
    state: &Rc<RefCell<AppState>>,
    path: String,
    mode: NavMode,
    done_message: Option<String>,
) {
    let show_hidden = state.borrow().show_hidden;
    // The tab this request belongs to; a completion for any other tab is dropped.
    let tab_id = state.borrow().tabs.active().id;
    let ui = ui_for_state(state);
    // Refreshing the SAME folder (e.g. after a delete) should keep the scroll
    // position; only real navigation to a different folder resets to the top.
    let same_folder = state.borrow().cwd == path;
    let saved_scroll = if same_folder {
        ui.as_ref().map(|ui| ui.list_scroll.vadjustment().value())
    } else {
        None
    };
    // A listing can block for seconds on a remote/FUSE mount. Do it on a worker
    // and give every request a generation and cancellation flag, so an old
    // response cannot replace a folder the user has already left.
    let (generation, cancelled) = {
        let mut state = state.borrow_mut();
        if let Some(previous) = state.listing_cancel.take() {
            previous.store(true, Ordering::Relaxed);
        }
        // The previous monitor belongs to the old listing. Drop it before the
        // new load completes so a late notification cannot cancel this request.
        state.directory_monitor = None;
        state.monitor_generation = state.monitor_generation.wrapping_add(1);
        if let Some(queue) = state.thumbnail_queue.take() {
            queue.borrow().cancelled.cancel();
        }
        state.listing_generation = state.listing_generation.wrapping_add(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        state.listing_cancel = Some(cancelled.clone());
        (state.listing_generation, cancelled)
    };

    if let Some(ui) = &ui {
        set_breadcrumbs(state, ui, &path);
        ui.path_stack.set_visible_child_name("label");
        ui.search.set_text("");
        if !same_folder {
            // D-34: clearing removes the focused row; hand focus to the view root
            // before GTK's next-frame focus search can reach the preview label.
            let focus_was_in_list = focus_within(&ui.window, &ui.list);
            let focus_was_in_grid = focus_within(&ui.window, &ui.grid);
            clear_file_views(ui);
            keep_focus_on_empty_view(&ui.list, focus_was_in_list, true);
            keep_focus_on_empty_view(&ui.grid, focus_was_in_grid, true);
        }
        set_preview_text(ui, "Loading folder…");
        ui.status.set_label(&format!("Loading {path}"));
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let path_for_worker = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(native_core::list_dir_cancellable(
            &path_for_worker,
            show_hidden,
            &cancelled,
        ));
    });

    let state = state.clone();
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
        Ok(Ok(listing)) => {
            // The result is only valid for the newest navigation request.
            if state.borrow().listing_generation != generation
                || state.borrow().tabs.active().id != tab_id
            {
                return glib::ControlFlow::Break;
            }
            let item_count = listing.entries.len();
            let metadata_unavailable = listing.metadata_unavailable;
            let unreadable_entries = listing.unreadable_entries;
            {
                let mut state = state.borrow_mut();
                state.listing_cancel = None;
                // Downloads is a chronological inbox. New files belong at the
                // top every time it is opened; clicking a column header still
                // immediately switches to that requested ordering.
                if is_downloads_dir(&path) {
                    state.sort_key = SortKey::Modified;
                    state.sort_ascending = false;
                }
                state.cwd = path.clone();
                let tab = state.tabs.active_mut();
                match mode {
                    NavMode::Record => tab.record(&path),
                    NavMode::Keep => tab.path = path.clone(),
                    NavMode::Cursor(index) => {
                        if tab.history.get(index) == Some(&path) {
                            tab.index = index;
                        }
                        tab.path = path.clone();
                    }
                }
                if state
                    .sidebar_active_key
                    .as_deref()
                    .and_then(|key| key.split_once('\n').map(|(_, active_path)| active_path))
                    .is_some_and(|active_path| active_path != path)
                {
                    state.sidebar_active_key = None;
                }
                state.entries = listing.entries;
                state.visible_entries.clear();
                state.filter_text.clear();
                match update_config(|config| push_recent(&mut config.recents, &path)) {
                    Some(config) => state.recents = config.recents,
                    None => push_recent(&mut state.recents, &path),
                }
            }
            if let Some(ui) = &ui {
                rebuild_tabs(&state, ui);
                set_breadcrumbs(&state, ui, &path);
                ui.path_stack.set_visible_child_name("label");
                ui.search.set_text("");
                set_preview_text(ui, "Select a file");
                repopulate_rows(&state, ui);
                update_selection(&state, ui);
                update_nav_buttons(&state, ui);
                update_sidebar_active_rows(&state, ui);
                install_directory_monitor(&state, ui, &path);
                if let Some(target) = state.borrow().pinned_top.clone() {
                    select_entry_by_path(&state, ui, &target);
                }
                ui.status.set_label(&listing_status(
                    item_count,
                    metadata_unavailable,
                    unreadable_entries,
                ));
                if let Some(message) = &done_message {
                    ui.status.set_label(message);
                }
                // Restore the pre-refresh scroll position once the rebuilt rows
                // have been allocated (so the vadjustment upper bound is valid).
                if let Some(value) = saved_scroll {
                    let adj = ui.list_scroll.vadjustment();
                    glib::timeout_add_local_once(Duration::from_millis(16), move || {
                        adj.set_value(value);
                    });
                }
            }
            glib::ControlFlow::Break
        }
        Ok(Err(native_core::ListDirError::Cancelled)) => glib::ControlFlow::Break,
        Ok(Err(error)) => {
            if state.borrow().listing_generation == generation {
                state.borrow_mut().listing_cancel = None;
                if let Some(ui) = &ui {
                    let current_path = state.borrow().cwd.clone();
                    set_breadcrumbs(&state, ui, &current_path);
                    repopulate_rows(&state, ui);
                    update_selection(&state, ui);
                    ui.status.set_label(&format!("Cannot open {path}: {error}"));
                }
            }
            glib::ControlFlow::Break
        }
        Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(TryRecvError::Disconnected) => {
            if state.borrow().listing_generation == generation {
                state.borrow_mut().listing_cancel = None;
                if let Some(ui) = &ui {
                    ui.status.set_label("Folder loading failed");
                }
            }
            glib::ControlFlow::Break
        }
    });
}

fn clear_file_views(ui: &Ui) {
    ui.list_store.remove_all();
    while let Some(child) = ui.grid.first_child() {
        ui.grid.remove(&child);
    }
}

fn install_directory_monitor(state: &Rc<RefCell<AppState>>, ui: &Ui, path: &str) {
    let monitor_generation = {
        let mut state = state.borrow_mut();
        state.directory_monitor = None;
        state.monitor_generation = state.monitor_generation.wrapping_add(1);
        state.monitor_generation
    };
    let file = gio::File::for_path(path);
    let Ok(monitor) = file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
    else {
        // Some network/FUSE mounts do not implement file-change notifications.
        // The folder remains usable and can still be refreshed with F5.
        return;
    };

    let watched_path = path.to_string();
    let coalescer = Rc::new(RefCell::new(native_core::RefreshCoalescer::default()));
    let state_weak = Rc::downgrade(state);
    let ui = ui.clone();
    monitor.connect_changed(move |_, _, _, event| {
        if !matches!(
            event,
            gio::FileMonitorEvent::Changed
                | gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Deleted
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::AttributeChanged
                | gio::FileMonitorEvent::Moved
                | gio::FileMonitorEvent::Renamed
                | gio::FileMonitorEvent::MovedIn
                | gio::FileMonitorEvent::MovedOut
                | gio::FileMonitorEvent::Unmounted
        ) {
            return;
        }
        let Some(state) = state_weak.upgrade() else {
            return;
        };
        if state.borrow().monitor_generation != monitor_generation
            || state.borrow().cwd != watched_path
        {
            return;
        }
        let Some(delay) = coalescer.borrow_mut().on_event(monotonic_ms()) else {
            return;
        };
        arm_monitor_scan(&state, &ui, &coalescer, monitor_generation, &watched_path, delay);
    });
    state.borrow_mut().directory_monitor = Some(monitor);
}

#[cfg(any(test))]
thread_local! {
    /// (start, end) of every monitor scan, in monotonic ms; end is 0 while running.
    static SCAN_LOG: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Arms the one timer the coalescer asked for; when it fires the scan starts.
fn arm_monitor_scan(
    state: &Rc<RefCell<AppState>>,
    ui: &Ui,
    coalescer: &Rc<RefCell<native_core::RefreshCoalescer>>,
    monitor_generation: u64,
    watched_path: &str,
    delay_ms: u64,
) {
    let state_weak = Rc::downgrade(state);
    let ui = ui.clone();
    let coalescer = coalescer.clone();
    let watched_path = watched_path.to_string();
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
        let Some(state) = state_weak.upgrade() else {
            return;
        };
        {
            let state_ref = state.borrow();
            if state_ref.monitor_generation != monitor_generation || state_ref.cwd != watched_path {
                return;
            }
        }
        coalescer.borrow_mut().on_timer(monotonic_ms());
        start_monitor_scan(&state, &ui, &coalescer, monitor_generation, watched_path);
    });
}

/// One background scan of the watched folder. It never goes through
/// navigation: no monitor reinstall, search reset or tab/breadcrumb rebuild.
fn start_monitor_scan(
    state: &Rc<RefCell<AppState>>,
    ui: &Ui,
    coalescer: &Rc<RefCell<native_core::RefreshCoalescer>>,
    monitor_generation: u64,
    path: String,
) {
    let show_hidden = state.borrow().show_hidden;
    #[cfg(any(test))]
    SCAN_LOG.with(|log| log.borrow_mut().push((monotonic_ms(), 0)));
    let (tx, rx) = std::sync::mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let path_for_worker = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(native_core::list_dir_cancellable(
            &path_for_worker,
            show_hidden,
            &cancelled,
        ));
    });
    let state = state.clone();
    let ui = ui.clone();
    let coalescer = coalescer.clone();
    glib::timeout_add_local(Duration::from_millis(40), move || {
        let result = match rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => None,
        };
        let current = {
            let state_ref = state.borrow();
            state_ref.monitor_generation == monitor_generation && state_ref.cwd == path
        };
        if current {
            if let Some(Ok(listing)) = result {
                apply_monitor_scan(&state, &ui, listing);
            }
            #[cfg(any(test))]
            SCAN_LOG.with(|log| {
                if let Some(last) = log.borrow_mut().last_mut() {
                    last.1 = monotonic_ms();
                }
            });
            if let Some(delay) = coalescer.borrow_mut().on_scan_done(monotonic_ms()) {
                arm_monitor_scan(&state, &ui, &coalescer, monitor_generation, &path, delay);
            }
        }
        glib::ControlFlow::Break
    });
}

/// Applies a monitor scan. Size/modified-only changes are merged in place so the
/// hovered and selected GTK objects survive; anything structural rebuilds the
/// rows and restores the surviving selection.
fn apply_monitor_scan(state: &Rc<RefCell<AppState>>, ui: &Ui, listing: native_core::DirectoryListing) {
    let kind = native_core::classify_refresh(&state.borrow().entries, &listing.entries);
    if kind == native_core::RefreshKind::MetadataOnly {
        let fresh: HashMap<&str, &Entry> =
            listing.entries.iter().map(|e| (e.path.as_str(), e)).collect();
        let merge = |entry: &mut Entry| -> bool {
            let Some(new) = fresh.get(entry.path.as_str()) else {
                return false;
            };
            let changed = entry.size != new.size || entry.modified != new.modified;
            entry.size = new.size;
            entry.modified = new.modified;
            changed
        };
        let mut changed_paths: Vec<Entry> = Vec::new();
        {
            let mut state_ref = state.borrow_mut();
            for entry in state_ref.entries.iter_mut() {
                merge(entry);
            }
            for entry in state_ref.visible_entries.iter_mut() {
                if merge(entry) {
                    changed_paths.push(entry.clone());
                }
            }
        }
        for index in 0..ui.list_store.n_items() {
            if let Some(boxed) = ui
                .list_store
                .item(index)
                .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
            {
                merge(&mut boxed.borrow_mut::<Entry>());
            }
        }
        let labels = state.borrow().meta_labels.clone();
        for entry in changed_paths {
            if let Some((size, modified)) = labels.get(&entry.path) {
                if let Some(size) = size.upgrade() {
                    size.set_label(&row_size_text(&entry));
                }
                if let Some(modified) = modified.upgrade() {
                    modified.set_label(&row_modified_text(&entry));
                }
            }
        }
        return;
    }

    let selected = selected_paths(state, ui);
    let saved_scroll = ui.list_scroll.vadjustment().value();
    let (item_count, metadata_unavailable, unreadable_entries) = (
        listing.entries.len(),
        listing.metadata_unavailable,
        listing.unreadable_entries,
    );
    state.borrow_mut().entries = listing.entries;
    repopulate_rows(state, ui);
    restore_selection(state, ui, &selected);
    update_selection(state, ui);
    ui.status.set_label(&listing_status(
        item_count,
        metadata_unavailable,
        unreadable_entries,
    ));
    let adj = ui.list_scroll.vadjustment();
    glib::timeout_add_local_once(Duration::from_millis(16), move || adj.set_value(saved_scroll));
}

/// Re-selects every surviving path after a structural rebuild.
fn restore_selection(state: &Rc<RefCell<AppState>>, ui: &Ui, paths: &[String]) {
    let (view_mode, indices): (ViewMode, Vec<usize>) = {
        let state = state.borrow();
        (
            state.view_mode,
            paths
                .iter()
                .filter_map(|path| state.visible_entries.iter().position(|e| &e.path == path))
                .collect(),
        )
    };
    if indices.is_empty() {
        return;
    }
    match view_mode {
        ViewMode::Grid => {
            ui.grid.unselect_all();
            for index in indices {
                if let Some(child) = ui.grid.child_at_index(index as i32) {
                    ui.grid.select_child(&child);
                }
            }
        }
        ViewMode::Details | ViewMode::Compact => {
            ui.list_selection.unselect_all();
            for index in indices {
                ui.list_selection.select_item(index as u32, false);
            }
        }
    }
}

fn listing_status(
    item_count: usize,
    metadata_unavailable: usize,
    unreadable_entries: usize,
) -> String {
    let mut message = format!("{item_count} item{} shown", plural(item_count));
    if metadata_unavailable > 0 {
        message.push_str(&format!(
            " · {metadata_unavailable} with unavailable metadata"
        ));
    }
    if unreadable_entries > 0 {
        message.push_str(&format!(
            " · {unreadable_entries} unreadable entr{}",
            if unreadable_entries == 1 { "y" } else { "ies" }
        ));
    }
    message
}

fn update_nav_buttons(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let state = state.borrow();
    ui.back.set_sensitive(state.tabs.active().can_back());
    ui.forward.set_sensitive(state.tabs.active().can_forward());
}

thread_local! {
    static WINDOWS: RefCell<native_core::WindowRegistry<AppState, Ui, gio::ApplicationHoldGuard>> =
        RefCell::new(native_core::WindowRegistry::default());
}

fn register_window(state: &Rc<RefCell<AppState>>, ui: &Ui, hold: gio::ApplicationHoldGuard) {
    WINDOWS.with(|windows| windows.borrow_mut().register(state, ui.clone(), hold));
}

/// Drops only this window's entry, which releases only its hold.
fn unregister_window(state: &Rc<RefCell<AppState>>) {
    #[cfg(feature = "guide")]
    {
        let mut st = state.borrow_mut();
        st.guide_watch_dir = None;
        st.guide_target = None;
        drop(st);
        guide_unpaint(state);
        let mut st = state.borrow_mut();
        if let Some(monitor) = st.guide_monitor.take() {
            monitor.cancel();
        }
    }
    let removed = WINDOWS.with(|windows| windows.borrow_mut().unregister(state));
    drop(removed);
}

fn ui_for_state(state: &Rc<RefCell<AppState>>) -> Option<Ui> {
    WINDOWS.with(|windows| windows.borrow().ui_for_state(state))
}

fn state_for_ui(ui: &Ui) -> Option<Rc<RefCell<AppState>>> {
    WINDOWS.with(|windows| windows.borrow().state_for_ui(|other| other.window == ui.window))
}

fn active_window() -> Option<(Rc<RefCell<AppState>>, Ui)> {
    WINDOWS.with(|windows| windows.borrow().active(|ui| ui.window.is_active()))
}

/// Rebuild the address bar as one clickable button per path segment.
/// Clicking "home" from /home/zito/Downloads jumps straight to /home.
fn set_breadcrumbs(state: &Rc<RefCell<AppState>>, ui: &Ui, path: &str) {
    while let Some(child) = ui.path_crumbs.first_child() {
        ui.path_crumbs.remove(&child);
    }
    ui.path_crumbs.set_tooltip_text(Some(path));

    // (label, path-to-navigate-to). "Computer" is the filesystem root.
    let mut crumbs: Vec<(String, String)> = vec![("Computer".to_string(), "/".to_string())];
    let mut accumulated = PathBuf::from("/");
    for component in PathBuf::from(path).components() {
        if let std::path::Component::Normal(value) = component {
            accumulated.push(value);
            crumbs.push((
                value.to_string_lossy().to_string(),
                accumulated.to_string_lossy().to_string(),
            ));
        }
    }

    let last = crumbs.len().saturating_sub(1);
    for (index, (label, target)) in crumbs.into_iter().enumerate() {
        if index > 0 {
            let sep = gtk::Label::new(Some("›"));
            sep.add_css_class("wf-crumb-sep");
            ui.path_crumbs.append(&sep);
        }
        let button = gtk::Button::builder().label(&label).build();
        button.add_css_class("flat");
        button.add_css_class("wf-crumb");
        if index == last {
            // The folder you are already in is not a link.
            button.add_css_class("wf-crumb-current");
        }
        let state = state.clone();
        button.connect_clicked(move |_| navigate(&state, target.clone()));
        ui.path_crumbs.append(&button);
    }
}


/// Builds and recycles the Details/Compact rows. `bind` runs only for rows that
/// are actually on screen, so this is called ~30 times regardless of whether the
/// folder holds ten files or a hundred thousand.
/// Re-binds one row (after an inline edit starts or is cancelled) and keeps it
/// selected; a ListStore change on a position drops that row's selection.
fn refresh_row(state: &Rc<RefCell<AppState>>, ui: &Ui, path: &str) {
    if state.borrow().view_mode == ViewMode::Grid {
        // Grid has no row model: rebuild its children and re-select the item.
        repopulate_rows(state, ui);
        select_entry_by_path(state, ui, path);
        return;
    }
    let Some(entry) = state.borrow().visible_entries.iter().find(|e| e.path == path).cloned() else {
        return;
    };
    let pos = state.borrow().visible_entries.iter().position(|e| e.path == path).unwrap_or(0);
    // A real replace, not items_changed: ListView only rebinds a row when the
    // model reports a different item there.
    ui.list_store.splice(pos as u32, 1, &[glib::BoxedAnyObject::new(entry)]);
    ui.list_selection.select_item(pos as u32, true);
}

fn begin_inline_rename(state: &Rc<RefCell<AppState>>, ui: &Ui, entry: &Entry) {
    state.borrow_mut().inline_rename = Some(InlineRename::new(&entry.path, &entry.name));
    state.borrow_mut().last_name_click = None;
    refresh_row(state, ui, &entry.path);
}

fn cancel_inline_rename(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let Some(edit) = state.borrow_mut().inline_rename.take() else {
        return;
    };
    refresh_row(state, ui, &edit.path);
}

/// The one commit path for Enter and focus loss. Success renames on disk,
/// clears the edit and reloads (the existing sort re-places the row); failure
/// keeps the same field open with the typed text and the reason in the status.
fn commit_inline_rename(state: &Rc<RefCell<AppState>>, ui: &Ui, field: &gtk::Entry) {
    let typed = field.text().trim().to_string();
    let path = {
        let mut st = state.borrow_mut();
        let Some(edit) = st.inline_rename.as_mut() else {
            return;
        };
        if !edit.begin_commit(&typed) {
            return;
        }
        edit.path.clone()
    };
    let old_name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if typed == old_name {
        cancel_inline_rename(state, ui);
        return;
    }
    match native_core::rename_path(&path, &typed) {
        Ok(to) => {
            state.borrow_mut().inline_rename = None;
            push_undo(state, UndoAction::Renamed { from: path, to: to.clone() });
            let cwd = state.borrow().cwd.clone();
            navigate(state, cwd);
            ui.status.set_label(&format!("Renamed {old_name} to {typed}"));
        }
        Err(err) => {
            if let Some(edit) = state.borrow_mut().inline_rename.as_mut() {
                edit.fail(err.clone());
            }
            ui.status.set_label(&format!("Cannot rename {old_name} to {typed}: {err}"));
            let field = field.clone();
            glib::idle_add_local_once(move || {
                field.grab_focus();
                field.select_region(0, -1);
            });
        }
    }
}

/// What the inline editor needs from its surroundings, so the GTK wiring can be
/// tested against real widgets without a whole window.
struct InlineHost {
    /// Enter: the only route that may write.
    commit: Rc<dyn Fn(&gtk::Entry)>,
    /// Escape, focus loss, click outside: restore the row, write nothing.
    cancel: Rc<dyn Fn()>,
    /// Whether this editor is still the live one.
    active: Rc<dyn Fn() -> bool>,
}

fn is_self_or_descendant(widget: &gtk::Widget, field: &gtk::Widget) -> bool {
    let mut cur = Some(widget.clone());
    while let Some(w) = cur {
        if &w == field {
            return true;
        }
        cur = w.parent();
    }
    false
}

fn wire_inline_editor(field: &gtk::Entry, window: &gtk::Window, host: InlineHost) {
    {
        let commit = host.commit.clone();
        field.connect_activate(move |f| {
            debug_assert!(inline_event_commits(InlineEvent::Enter));
            eprintln!("wf-event: inline editor Enter");
            commit(f)
        });
    }
    let keys = gtk::EventControllerKey::new();
    {
        let cancel = host.cancel.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                debug_assert!(!inline_event_commits(InlineEvent::Escape));
                eprintln!("wf-event: inline editor Escape");
                cancel();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    field.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    {
        let cancel = host.cancel.clone();
        // Clicking away cancels (owner decision); Enter is the only commit.
        focus.connect_leave(move |_| {
            debug_assert!(!inline_event_commits(InlineEvent::FocusLoss));
            eprintln!("wf-event: inline editor focus loss");
            cancel();
        });
    }
    field.add_controller(focus);
    // Own pointer events: clicks inside the editor place the caret and must not
    // reach the row's drag/selection handling.
    let click = gtk::GestureClick::new();
    click.connect_pressed(|g, _, _, _| { g.set_state(gtk::EventSequenceState::Claimed); });
    field.add_controller(click);

    // A click on empty list space does not always move keyboard focus, so focus
    // loss alone would miss it: also watch presses anywhere in the window and
    // cancel when one lands outside the field. Controllers are never removed
    // during event dispatch: removal is deferred, and also happens when the
    // field unrealizes, so no watcher outlives its row.
    let watcher = gtk::GestureClick::new();
    watcher.set_propagation_phase(gtk::PropagationPhase::Capture);
    let remove_later = {
        let window = window.clone();
        let this = watcher.clone();
        let removed = Rc::new(std::cell::Cell::new(false));
        Rc::new(move || {
            if removed.replace(true) {
                return;
            }
            let (window, this) = (window.clone(), this.clone());
            glib::idle_add_local_once(move || window.remove_controller(&this));
        })
    };
    {
        let (field, host_cancel, active, remove_later) =
            (field.clone(), host.cancel.clone(), host.active.clone(), remove_later.clone());
        let window = window.clone();
        watcher.connect_pressed(move |_, _, x, y| {
            if !active() || !field.is_mapped() {
                remove_later();
                return;
            }
            let inside = window
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .is_some_and(|w| is_self_or_descendant(&w, field.upcast_ref()));
            if !inside {
                eprintln!("wf-event: inline editor outside press");
                remove_later();
                host_cancel();
            }
        });
    }
    field.connect_unrealize(move |_| remove_later());
    window.add_controller(watcher);
}

fn build_inline_editor(
    state: &Rc<RefCell<AppState>>,
    ui: &Ui,
    entry: &Entry,
    edit: &InlineRename,
) -> gtk::Entry {
    let field = gtk::Entry::builder().text(&edit.text).hexpand(true).build();
    field.add_css_class("wf-file-name");
    let path = edit.path.clone();
    wire_inline_editor(
        &field,
        ui.window.upcast_ref::<gtk::Window>(),
        InlineHost {
            commit: {
                let (state, ui) = (state.clone(), ui.clone());
                Rc::new(move |f| commit_inline_rename(&state, &ui, f))
            },
            cancel: {
                let (state, ui) = (state.clone(), ui.clone());
                Rc::new(move || cancel_inline_rename(&state, &ui))
            },
            active: {
                let state = state.clone();
                Rc::new(move || {
                    state.borrow().inline_rename.as_ref().is_some_and(|e| e.path == path)
                })
            },
        },
    );

    let selection = if entry.is_dir { NameSelection::All } else { NameSelection::StemOnly };
    let end = match selection {
        NameSelection::All => -1,
        NameSelection::StemOnly => {
            let (stem, ext) = split_stem_ext(&edit.text);
            if ext.is_empty() { -1 } else { stem.chars().count() as i32 }
        }
    };
    let f = field.clone();
    glib::idle_add_local_once(move || {
        f.grab_focus();
        f.select_region(0, end);
    });
    field
}

/// Slow second click on an already-selected Name starts the inline edit.
fn connect_name_slow_click(
    label: &gtk::Label,
    state: &Rc<RefCell<AppState>>,
    ui: &Ui,
    path: &str,
) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let (state, ui, path) = (state.clone(), ui.clone(), path.to_string());
    click.connect_released(move |_, n_press, _, _| {
        let prior = state
            .borrow()
            .last_name_click
            .as_ref()
            .filter(|(p, _)| *p == path)
            .map(|(_, at)| at.elapsed());
        let only_this = selected_paths(&state, &ui) == vec![path.clone()];
        state.borrow_mut().last_name_click = Some((path.clone(), std::time::Instant::now()));
        if slow_click_starts_rename(prior, n_press, only_this) {
            let entry = state.borrow().visible_entries.iter().find(|e| e.path == path).cloned();
            if let Some(entry) = entry {
                begin_inline_rename(&state, &ui, &entry);
            }
        }
    });
    label.add_controller(click);
}

fn row_size_text(entry: &Entry) -> String {
    if entry.metadata_error.is_some() {
        "—".to_string()
    } else if entry.is_dir {
        String::new()
    } else {
        native_core::fmt_size(entry.size)
    }
}

fn row_modified_text(entry: &Entry) -> String {
    entry
        .metadata_error
        .as_ref()
        .map(|_| "unavailable".to_string())
        .unwrap_or_else(|| native_core::fmt_modified(entry.modified))
}

fn connect_list_factory(factory: &gtk::SignalListItemFactory, state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let state = state.clone();
    let state_for_unbind = state.clone();
    let ui_for_unbind = ui.clone();
    let ui = ui.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item.item().and_then(|i| i.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let entry: Entry = boxed.borrow::<Entry>().clone();
        let icon_zoom = state.borrow().icon_zoom;

        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.add_css_class("wf-file-row");
        line.set_tooltip_text(Some(&entry.metadata_error.as_ref().map_or_else(
            || entry.path.clone(),
            |error| format!("{}\nMetadata unavailable: {error}", entry.path),
        )));

        let icon = file_icon(&entry, icon_zoom);
        // Show full file names: short names stay on one line (compact); long names
        // wrap to a second line rather than being truncated. Only names too long
        // for two lines fall back to an ellipsis.
        let name = gtk::Label::builder()
            .label(&entry.name)
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("wf-file-name");
        let editing = state
            .borrow()
            .inline_rename
            .as_ref()
            .filter(|edit| edit.path == entry.path)
            .cloned();
        let name: gtk::Widget = match editing {
            Some(edit) => build_inline_editor(&state, &ui, &entry, &edit).upcast(),
            None => {
                connect_name_slow_click(&name, &state, &ui, &entry.path);
                name.upcast()
            }
        };
        let size_text = row_size_text(&entry);
        // max_width_chars is deliberately small on all three: it only sets the
        // shrink floor. The real width comes from the header's size_request via
        // the shared SizeGroups, so dragging a column resizes every row with it.
        let size = gtk::Label::builder()
            .label(&size_text)
            .xalign(1.0)
            .max_width_chars(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        size.add_css_class("wf-file-meta");
        ui.columns.size.add_widget(&size);
        let kind = gtk::Label::builder()
            .label(&native_core::kind_label(&entry))
            .xalign(0.0)
            .max_width_chars(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        kind.add_css_class("wf-file-meta");
        ui.columns.kind.add_widget(&kind);
        let modified_text = row_modified_text(&entry);
        let modified = gtk::Label::builder()
            .label(&modified_text)
            .xalign(0.0)
            .max_width_chars(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        modified.add_css_class("wf-file-meta");
        ui.columns.modified.add_widget(&modified);
        state
            .borrow_mut()
            .meta_labels
            .insert(entry.path.clone(), (size.downgrade(), modified.downgrade()));

        line.append(&icon);
        line.append(&name);
        line.append(&size);
        line.append(&kind);
        line.append(&modified);

        {
            // Records which file each press hit, for the double-click gate. It
            // never claims the sequence.
            let press = gtk::GestureClick::new();
            press.set_button(1);
            press.set_propagation_phase(gtk::PropagationPhase::Capture);
            let gate = state.borrow().activation_gate.clone();
            let path = entry.path.clone();
            press.connect_pressed(move |g, _, _, _| {
                let plain = !g.current_event_state().intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::SHIFT_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK
                        | gdk::ModifierType::META_MASK,
                );
                gate.borrow_mut().press(&path, std::time::Instant::now(), plain);
            });
            line.add_controller(press);
        }
        connect_file_drag(&line, state.clone(), ui.clone(), entry.path.clone(), Some(&size));
        if entry.is_dir {
            connect_file_drop_to_dir(&line, state.clone(), ui.clone(), entry.path.clone());
        }
        connect_row_context(&line, item.position(), state.clone(), ui.clone(), entry.clone());
        if entry.is_dir {
            // Middle-click a folder to open it in a new tab, like a browser.
            let middle = gtk::GestureClick::new();
            middle.set_button(2);
            let state_for_middle = state.clone();
            let ui_for_middle = ui.clone();
            let path = entry.path.clone();
            middle.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                open_path_in_new_tab(&state_for_middle, &ui_for_middle, path.clone());
            });
            line.add_controller(middle);
        }

        #[cfg(feature = "guide")]
        if guide_is_open_file_fixture(&entry) {
            guide_name(&line, GUIDE_OPEN_FILE);
            guide_resolve_open_file_later(&state);
        }
        item.set_child(Some(&line));
    });
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        // A SizeGroup holds a strong reference to every widget added to it. Rows
        // are rebuilt on each bind and rebound constantly while scrolling, so
        // without this the three column groups would grow without bound, pinning
        // every row ever displayed in memory and dragging dead widgets into each
        // width calculation.
        if let Some(line) = item.child().and_then(|c| c.downcast::<gtk::Box>().ok()) {
            let mut children = Vec::new();
            let mut child = line.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                children.push(widget);
            }
            // Order matches the bind above: icon, name, size, kind, modified.
            if children.len() >= 5 {
                if let Some(path) = item
                    .item()
                    .and_then(|i| i.downcast::<glib::BoxedAnyObject>().ok())
                    .map(|b| b.borrow::<Entry>().path.clone())
                {
                    let mut state = state_for_unbind.borrow_mut();
                    let ours = state.meta_labels.get(&path).is_some_and(|(size, _)| {
                        size.upgrade().is_some_and(|l| l.upcast_ref::<gtk::Widget>() == &children[2])
                    });
                    if ours {
                        state.meta_labels.remove(&path);
                    }
                }
                ui_for_unbind.columns.size.remove_widget(&children[2]);
                ui_for_unbind.columns.kind.remove_widget(&children[3]);
                ui_for_unbind.columns.modified.remove_widget(&children[4]);
            }
        }
        #[cfg(feature = "guide")]
        if guide_waits_for_open_file(&state_for_unbind)
            && item.child().is_some_and(|c| c.widget_name() == GUIDE_OPEN_FILE)
        {
            // The visual's label is parented to this row: drop it before the row goes.
            guide_unpaint(&state_for_unbind);
            guide_resolve_open_file_later(&state_for_unbind);
        }
        item.set_child(None::<&gtk::Widget>);
    });
}

/// Indices of the selected rows, in order.
fn selected_indices(ui: &Ui) -> Vec<usize> {
    let bitset = ui.list_selection.selection();
    let mut out = Vec::new();
    let size = bitset.size();
    for i in 0..size {
        out.push(bitset.nth(i as u32) as usize);
    }
    out
}

fn repopulate_rows(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    #[cfg(feature = "guide")]
    {
        guide_sync_crucial(state, ui);
        // Rows and grid children are rebuilt: no visual may outlive its old target.
        if guide_waits_for_open_file(state) {
            guide_unpaint(state);
            guide_resolve_open_file_later(state);
        }
    }
    let focus_was_in_files = focus_within(&ui.window, &ui.list) || focus_within(&ui.window, &ui.grid);
    ui.list_store.remove_all();
    while let Some(child) = ui.grid.first_child() {
        ui.grid.remove(&child);
    }

    let (entries, filter_text, sort_key, sort_ascending, view_mode, icon_zoom) = {
        let state = state.borrow();
        (
            state.entries.clone(),
            state.filter_text.to_lowercase(),
            state.sort_key,
            state.sort_ascending,
            state.view_mode,
            state.icon_zoom,
        )
    };
    let mut visible_entries: Vec<Entry> = if filter_text.is_empty() {
        entries
    } else {
        entries
            .into_iter()
            .filter(|entry| entry.name.to_lowercase().contains(&filter_text))
            .collect()
    };
    sort_entries(&mut visible_entries, sort_key, sort_ascending);

    // Force the pinned path (a just-downloaded, just-opened file) to the very top,
    // ahead of folders and every sort key.
    let pinned = state.borrow().pinned_top.clone();
    if let Some(pin) = pinned {
        if let Some(pos) = visible_entries.iter().position(|entry| entry.path == pin) {
            let entry = visible_entries.remove(pos);
            visible_entries.insert(0, entry);
        }
    }

    let thumbnail_queue = {
        let mut state_ref = state.borrow_mut();
        if let Some(previous) = state_ref.thumbnail_queue.take() {
            previous.borrow().cancelled.cancel();
        }
        state_ref.visible_entries = visible_entries.clone();
        (view_mode == ViewMode::Grid).then(|| {
            let queue = Rc::new(RefCell::new(ThumbnailQueue {
                cancelled: gio::Cancellable::new(),
                pending: VecDeque::new(),
                active: 0,
            }));
            state_ref.thumbnail_queue = Some(queue.clone());
            queue
        })
    };
    let thumbnail_size = (
        ((58.0 * icon_zoom).round() as i32).max(1) * 2,
        ((48.0 * icon_zoom).round() as i32).max(1) * 2,
    );

    ui.list_placeholder.set_label(if !filter_text.is_empty() {
        "No items match your search"
    } else {
        "This folder is empty"
    });

    // The list is virtualized: hand the model the whole set and let ListView
    // realize only what is on screen. This is O(1) in row count — the 21,436-row
    // folder costs the same here as a 10-row one.
    ui.list_placeholder.set_visible(visible_entries.is_empty());
    if view_mode != ViewMode::Grid {
        ui.list_store.remove_all();
        let items: Vec<glib::BoxedAnyObject> = visible_entries
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        ui.list_store.splice(0, 0, &items);
    } else {
        ui.list_store.remove_all();
    }
    if view_mode == ViewMode::Grid {
        keep_focus_on_empty_view(&ui.grid, focus_was_in_files, visible_entries.is_empty());
    } else {
        keep_focus_on_empty_view(&ui.list, focus_was_in_files, visible_entries.is_empty());
    }

    // Only Grid still builds a widget per item; Details/Compact are served by the
    // virtualized ListView above and must not walk the whole listing here.
    // NOTE: Grid remains un-virtualized, so a huge folder is still slow in Icons view.
    if view_mode != ViewMode::Grid {
        apply_pending_reveal(state, ui);
        return;
    }

    for (index, entry) in visible_entries.into_iter().enumerate() {
        let tooltip = entry.metadata_error.as_ref().map_or_else(
            || entry.path.clone(),
            |error| format!("{}\nMetadata unavailable: {error}", entry.path),
        );

        if view_mode == ViewMode::Grid {
            let grid_child = gtk::FlowBoxChild::builder().tooltip_text(&tooltip).build();
            grid_child.add_css_class("wf-grid-child");
            #[cfg(feature = "guide")]
            if guide_is_open_file_fixture(&entry) {
                guide_name(&grid_child, GUIDE_OPEN_FILE);
            }
            let grid_item = gtk::Box::new(gtk::Orientation::Vertical, 4);
            grid_item.add_css_class("wf-grid-item");
            grid_item.set_halign(gtk::Align::Center);
            grid_item.set_valign(gtk::Align::Start);

            let wants_thumbnail = thumbnail_queue.is_some() && index < MAX_GRID_THUMBNAILS;
            let (icon, thumbnail) = grid_file_icon(&entry, icon_zoom, wants_thumbnail);
            let name = gtk::Label::builder()
                .label(&entry.name)
                .xalign(0.5)
                .justify(gtk::Justification::Center)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .max_width_chars(12)
                .width_chars(12)
                .lines(2)
                .wrap(true)
                .build();
            name.add_css_class("wf-grid-name");
            grid_item.append(&icon);
            let editing = state
                .borrow()
                .inline_rename
                .as_ref()
                .filter(|edit| edit.path == entry.path)
                .cloned();
            match editing {
                Some(edit) => {
                    let field = build_inline_editor(state, ui, &entry, &edit);
                    field.set_width_chars(12);
                    field.set_max_width_chars(12);
                    field.set_hexpand(false);
                    grid_item.append(&field);
                }
                None => grid_item.append(&name),
            }
            grid_child.set_child(Some(&grid_item));
            connect_file_drag(&grid_child, state.clone(), ui.clone(), entry.path.clone(), None);
            if entry.is_dir {
                connect_file_drop_to_dir(
                    &grid_child,
                    state.clone(),
                    ui.clone(),
                    entry.path.clone(),
                );
            }
            ui.grid.append(&grid_child);
            if let (Some(queue), Some(picture)) = (&thumbnail_queue, thumbnail) {
                queue.borrow_mut().pending.push_back(ThumbnailJob {
                    path: entry.path.clone(),
                    picture,
                    width: thumbnail_size.0,
                    height: thumbnail_size.1,
                });
            }
        }
    }
    if let Some(queue) = thumbnail_queue {
        start_thumbnail_jobs(queue);
    }
    apply_pending_reveal(state, ui);
}

/// Selects and scrolls to a just-created path once its row exists. Focus is left
/// to the inline editor, which claims it itself.
fn apply_pending_reveal(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let Some(path) = state.borrow_mut().reveal_path.take() else {
        return;
    };
    let index = state.borrow().visible_entries.iter().position(|e| e.path == path);
    let Some(index) = index else {
        return;
    };
    let view_mode = state.borrow().view_mode;
    match view_mode {
        ViewMode::Grid => {
            ui.grid.unselect_all();
            if let Some(child) = ui.grid.child_at_index(index as i32) {
                ui.grid.select_child(&child);
            }
        }
        ViewMode::Details | ViewMode::Compact => {
            ui.list_selection.select_item(index as u32, true);
            ui.list.scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
        }
    }
    update_selection(state, ui);
}

fn sort_entries(entries: &mut [Entry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        // Explorer parity: folders always sort ABOVE files, in every sort order.
        let dir_order = b.is_dir.cmp(&a.is_dir);
        if dir_order != std::cmp::Ordering::Equal {
            return dir_order;
        }
        let ordering = match key {
            SortKey::Name => natural_name_cmp(&a.name, &b.name),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Kind => native_core::kind_label(a).cmp(&native_core::kind_label(b)),
            SortKey::Modified => a.modified.cmp(&b.modified),
        };
        if ascending {
            ordering.then_with(|| natural_name_cmp(&a.name, &b.name))
        } else {
            ordering
                .reverse()
                .then_with(|| natural_name_cmp(&a.name, &b.name))
        }
    });
}

/// Explorer parity: case-insensitive compare where digit runs order numerically,
/// so "file2" sorts before "file10".
fn natural_name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.chars().flat_map(char::to_lowercase).peekable();
    let mut ib = b.chars().flat_map(char::to_lowercase).peekable();
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(c) = ia.peek().copied().filter(char::is_ascii_digit) {
                        na.push(c);
                        ia.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = ib.peek().copied().filter(char::is_ascii_digit) {
                        nb.push(c);
                        ib.next();
                    }
                    let va: u128 = na.parse().unwrap_or(u128::MAX);
                    let vb: u128 = nb.parse().unwrap_or(u128::MAX);
                    match va.cmp(&vb).then(na.len().cmp(&nb.len())) {
                        std::cmp::Ordering::Equal => {}
                        other => return other,
                    }
                } else {
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => {
                            ia.next();
                            ib.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

fn file_icon(entry: &Entry, zoom: f64) -> gtk::Box {
    let slot_w = (32.0 * zoom).round() as i32;
    let slot_h = (28.0 * zoom).round() as i32;
    let icon_w = (28.0 * zoom).round() as i32;
    let icon_h = (24.0 * zoom).round() as i32;
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.add_css_class("wf-file-icon");
    slot.set_size_request(slot_w, slot_h);
    slot.set_halign(gtk::Align::Center);
    slot.set_valign(gtk::Align::Center);

    // Use gtk::Image, never gtk::Picture, for the type icon. `set_size_request`
    // is a MINIMUM: a Picture grows to fill whatever space it is given, so rows
    // with a filename long enough to wrap onto two lines got a taller row and a
    // visibly bigger icon than their single-line neighbours. `set_pixel_size` on
    // an Image is an exact size, so every row's icon matches.
    let icon = icon_image(&entry.icon);
    icon.set_pixel_size(icon_w.min(icon_h));
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_hexpand(false);
    icon.set_vexpand(false);
    slot.append(&icon);

    slot
}

fn grid_file_icon(
    entry: &Entry,
    zoom: f64,
    wants_thumbnail: bool,
) -> (gtk::Box, Option<gtk::Picture>) {
    let slot_w = (58.0 * zoom).round() as i32;
    let slot_h = (48.0 * zoom).round() as i32;
    let icon_w = (44.0 * zoom).round() as i32;
    let icon_h = (38.0 * zoom).round() as i32;
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.set_size_request(slot_w, slot_h);
    slot.set_halign(gtk::Align::Center);
    slot.set_valign(gtk::Align::Center);

    if entry.is_image && wants_thumbnail {
        // Keep a normal file-type icon visible while the thumbnail is decoded
        // asynchronously. The queue limits work to four files at a time and is
        // cancelled as soon as the user changes folder or view.
        let overlay = gtk::Overlay::new();
        overlay.set_size_request(slot_w, slot_h);
        let placeholder = gtk::Image::from_icon_name(&entry.icon);
        placeholder.set_pixel_size(icon_w);
        placeholder.set_halign(gtk::Align::Center);
        placeholder.set_valign(gtk::Align::Center);
        overlay.set_child(Some(&placeholder));
        let thumb = gtk::Picture::new();
        thumb.set_can_shrink(true);
        thumb.set_content_fit(gtk::ContentFit::Cover);
        thumb.set_size_request(slot_w, slot_h);
        thumb.set_halign(gtk::Align::Center);
        thumb.set_valign(gtk::Align::Center);
        overlay.add_overlay(&thumb);
        slot.append(&overlay);
        return (slot, Some(thumb));
    }

    // Same fixed-size rule as the list view: an Image at an exact pixel size,
    // not a Picture that grows into whatever space the tile happens to have.
    let icon = icon_image(&entry.icon);
    icon.set_pixel_size(icon_w.min(icon_h));
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_hexpand(false);
    icon.set_vexpand(false);
    slot.append(&icon);

    (slot, None)
}

fn start_thumbnail_jobs(queue: Rc<RefCell<ThumbnailQueue>>) {
    loop {
        let (job, cancelled) = {
            let mut queue_ref = queue.borrow_mut();
            if queue_ref.cancelled.is_cancelled() {
                queue_ref.pending.clear();
                return;
            }
            if queue_ref.active >= MAX_CONCURRENT_THUMBNAILS {
                return;
            }
            let Some(job) = queue_ref.pending.pop_front() else {
                return;
            };
            queue_ref.active += 1;
            (job, queue_ref.cancelled.clone())
        };
        let queue_for_read = queue.clone();
        let picture = job.picture.clone();
        let width = job.width;
        let height = job.height;
        let file = gio::File::for_path(job.path);
        let cancelled_for_read = cancelled.clone();
        file.read_async(glib::Priority::DEFAULT, Some(&cancelled), move |stream| {
            let Ok(stream) = stream else {
                finish_thumbnail_job(&queue_for_read);
                return;
            };
            let queue_for_decode = queue_for_read.clone();
            let cancelled_for_decode = cancelled_for_read.clone();
            let cancelled_for_result = cancelled_for_decode.clone();
            Pixbuf::from_stream_at_scale_async(
                &stream,
                width,
                height,
                true,
                Some(&cancelled_for_decode),
                move |pixbuf| {
                    if let Ok(pixbuf) = pixbuf {
                        if !cancelled_for_result.is_cancelled() {
                            #[allow(deprecated)]
                            let texture = gdk::Texture::for_pixbuf(&pixbuf);
                            picture.set_paintable(Some(&texture));
                        }
                    }
                    finish_thumbnail_job(&queue_for_decode);
                },
            );
        });
    }
}

fn finish_thumbnail_job(queue: &Rc<RefCell<ThumbnailQueue>>) {
    {
        let mut queue_ref = queue.borrow_mut();
        queue_ref.active = queue_ref.active.saturating_sub(1);
        if queue_ref.cancelled.is_cancelled() {
            queue_ref.pending.clear();
            return;
        }
    }
    start_thumbnail_jobs(queue.clone());
}

thread_local! {
    /// Resolved icon-name -> file path. The lookup probes up to 560 paths on a
    /// miss, and a folder full of one file type asks for the same name on every
    /// single row.
    static ICON_PATH_CACHE: RefCell<HashMap<String, Option<PathBuf>>> =
        RefCell::new(HashMap::new());
    /// Decoded icon file -> texture. THIS is the one that mattered: building a
    /// gtk::Image with `from_file` decodes the image on every call, so a folder
    /// of 21,436 files decoded the same handful of icons 21,436 times. A texture
    /// is a GObject, so cloning it is a refcount bump and every row shares one
    /// decode.
    static ICON_TEXTURE_CACHE: RefCell<HashMap<PathBuf, Option<gdk::Texture>>> =
        RefCell::new(HashMap::new());
}

/// A ready-to-display icon widget, decoded at most once per distinct icon.
fn icon_image(icon_name: &str) -> gtk::Image {
    let texture = old_wayfinder_icon_path(icon_name).and_then(|path| {
        ICON_TEXTURE_CACHE.with(|cache| {
            cache
                .borrow_mut()
                .entry(path.clone())
                .or_insert_with(|| gdk::Texture::from_filename(&path).ok())
                .clone()
        })
    });
    match texture {
        Some(texture) => gtk::Image::from_paintable(Some(&texture)),
        None => gtk::Image::from_icon_name(icon_name),
    }
}

fn old_wayfinder_icon_path(name: &str) -> Option<PathBuf> {
    ICON_PATH_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| old_wayfinder_icon_path_inner(name))
            .clone()
    })
}

fn old_wayfinder_icon_path_inner(name: &str) -> Option<PathBuf> {
    const THEMES: [&str; 4] = ["breeze-dark", "breeze", "hicolor", "Adwaita"];
    const CATS: [&str; 5] = ["mimetypes", "places", "apps", "devices", "status"];
    const SIZES: [&str; 7] = ["64", "48", "32", "24", "22", "16", "scalable"];
    let mut names = vec![name];
    names.extend(["application-octet-stream", "text-x-generic", "unknown"]);

    for icon_name in names {
        for theme in THEMES {
            for category in CATS {
                for size in SIZES {
                    let path = PathBuf::from(format!(
                        "/usr/share/icons/{theme}/{category}/{size}/{icon_name}.svg"
                    ));
                    if Path::new(&path).exists() {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

fn selected_entries(state: &Rc<RefCell<AppState>>, ui: &Ui) -> Vec<Entry> {
    let state = state.borrow();
    match state.view_mode {
        ViewMode::Grid => ui
            .grid
            .selected_children()
            .into_iter()
            .filter_map(|child| {
                let idx = child.index();
                (idx >= 0)
                    .then_some(idx as usize)
                    .and_then(|idx| state.visible_entries.get(idx).cloned())
            })
            .collect(),
        ViewMode::Details | ViewMode::Compact => selected_indices(ui)
            .into_iter()
            .filter_map(|idx| state.visible_entries.get(idx).cloned())
            .collect(),
    }
}

fn selected_paths(state: &Rc<RefCell<AppState>>, ui: &Ui) -> Vec<String> {
    selected_entries(state, ui)
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

fn picker_entry_can_submit(picker: &PickerState, entry: &Entry) -> bool {
    if picker.config.directory {
        entry.is_dir
    } else {
        !entry.is_dir && picker_filter_allows(Some(picker), entry)
    }
}

fn picker_selected_paths(state: &Rc<RefCell<AppState>>, ui: &Ui) -> Vec<String> {
    let Some(picker) = state.borrow().picker.clone() else {
        return Vec::new();
    };
    let mut paths = selected_entries(state, ui)
        .into_iter()
        .filter(|entry| picker_entry_can_submit(&picker, entry))
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    if !picker.config.multiple {
        paths.truncate(1);
    }
    paths
}


fn entry_for_grid_child(state: &Rc<RefCell<AppState>>, child: &gtk::FlowBoxChild) -> Option<Entry> {
    let idx = child.index();
    if idx < 0 {
        return None;
    }
    state.borrow().visible_entries.get(idx as usize).cloned()
}

fn update_selection(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let entries = selected_entries(state, ui);
    update_picker_open_button(state, ui, &entries);
    match entries.as_slice() {
        [] => set_preview_text(ui, "Select a file"),
        [entry] => update_preview(ui, entry),
        entries => {
            let size: u64 = entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| entry.size)
                .sum();
            set_preview_text(
                ui,
                &format!(
                    "{} selected\n{}",
                    entries.len(),
                    native_core::fmt_size(size)
                ),
            );
        }
    }

    let state_ref = state.borrow();
    let mut status = if state_ref.filter_text.is_empty() {
        format!("{} items", state_ref.entries.len())
    } else {
        format!(
            "{} items  -  {} shown",
            state_ref.entries.len(),
            state_ref.visible_entries.len()
        )
    };
    if !entries.is_empty() {
        let selected_bytes: u64 = entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.size)
            .sum();
        if selected_bytes > 0 {
            status.push_str(&format!(
                "  -  {} selected  {}",
                entries.len(),
                native_core::fmt_size(selected_bytes)
            ));
        } else {
            status.push_str(&format!("  -  {} selected", entries.len()));
        }
    }
    if let Some(clipboard) = &state_ref.clipboard {
        let mode = match clipboard.mode {
            ClipboardMode::Copy => "copy",
            ClipboardMode::Cut => "cut",
        };
        status.push_str(&format!("  -  {} {} pending", clipboard.paths.len(), mode));
    }
    if !state_ref.undo_stack.is_empty() {
        status.push_str("  -  undo available");
    }
    ui.status.set_label(&status);
}

fn update_picker_open_button(state: &Rc<RefCell<AppState>>, ui: &Ui, entries: &[Entry]) {
    let Some(button) = &ui.picker_open else {
        return;
    };
    let Some(picker) = state.borrow().picker.clone() else {
        return;
    };
    let count = entries
        .iter()
        .filter(|entry| picker_entry_can_submit(&picker, entry))
        .count();
    let count = if picker.config.multiple {
        count
    } else {
        count.min(1)
    };
    button.set_sensitive(count > 0);
    if count > 1 {
        button.set_label(&format!("Open {count}"));
    } else if picker.config.directory {
        button.set_label("Open folder");
    } else {
        button.set_label("Open");
    }
}

fn connect_picker_actions(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let Some(button) = ui.picker_open.clone() else {
        return;
    };
    button.set_sensitive(false);
    {
        let state = state.clone();
        let ui = ui.clone();
        button.connect_clicked(move |_| {
            let Some(picker) = state.borrow().picker.clone() else {
                return;
            };
            let paths = picker_selected_paths(&state, &ui);
            if paths.is_empty() {
                ui.status.set_label(if picker.config.directory {
                    "Select a folder to open"
                } else {
                    "Select a file to open"
                });
                return;
            }
            picker_write_and_exit(&picker, paths, 0);
        });
    }
    let entries = selected_entries(state, ui);
    update_picker_open_button(state, ui, &entries);
}

fn open_selected(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    if let Some(entry) = selected_entries(state, ui).into_iter().next() {
        open_entry(state, ui, entry);
    }
}

fn move_selection(state: &Rc<RefCell<AppState>>, ui: &Ui, delta: isize) {
    let (mode, len) = {
        let state = state.borrow();
        (state.view_mode, state.visible_entries.len())
    };
    if len == 0 {
        return;
    }
    let current = match mode {
        ViewMode::Grid => ui
            .grid
            .selected_children()
            .first()
            .map(|child| child.index())
            .unwrap_or(-1),
        ViewMode::Details | ViewMode::Compact => selected_indices(ui)
            .first()
            .map(|idx| *idx as i32)
            .unwrap_or(-1),
    };
    let next = if current < 0 {
        0
    } else {
        (current as isize + delta).clamp(0, len.saturating_sub(1) as isize) as i32
    };
    select_index(state, ui, next as usize);
}

fn typeahead_select(state: &Rc<RefCell<AppState>>, ui: &Ui, ch: char) {
    let now = Instant::now();
    let needle = {
        let mut state_ref = state.borrow_mut();
        if now.duration_since(state_ref.typeahead_at) > Duration::from_millis(800) {
            state_ref.typeahead.clear();
        }
        state_ref.typeahead.push(ch.to_ascii_lowercase());
        state_ref.typeahead_at = now;
        state_ref.typeahead.clone()
    };
    let index = state
        .borrow()
        .visible_entries
        .iter()
        .position(|entry| entry.name.to_lowercase().starts_with(&needle));
    if let Some(index) = index {
        select_index(state, ui, index);
    }
}

/// Selects exactly the visible rows that are not selected now, in the active
/// presentation, without rebuilding the listing.
fn invert_selection(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let (view_mode, count) = {
        let state = state.borrow();
        (state.view_mode, state.visible_entries.len())
    };
    match view_mode {
        ViewMode::Grid => {
            for index in 0..count {
                if let Some(child) = ui.grid.child_at_index(index as i32) {
                    if child.is_selected() {
                        ui.grid.unselect_child(&child);
                    } else {
                        ui.grid.select_child(&child);
                    }
                }
            }
        }
        ViewMode::Details | ViewMode::Compact => {
            let count = (count as u32).min(ui.list_selection.n_items());
            let complement = gtk::Bitset::new_range(0, count);
            complement.subtract(&ui.list_selection.selection());
            ui.list_selection.set_selection(&complement, &gtk::Bitset::new_range(0, count));
        }
    }
    update_selection(state, ui);
}

fn select_index(state: &Rc<RefCell<AppState>>, ui: &Ui, index: usize) {
    // Copy the mode out: grabbing focus below can run editor handlers that
    // borrow the state again.
    let view_mode = state.borrow().view_mode;
    match view_mode {
        ViewMode::Grid => {
            ui.grid.unselect_all();
            if let Some(child) = ui.grid.child_at_index(index as i32) {
                ui.grid.select_child(&child);
                child.grab_focus();
            }
        }
        ViewMode::Details | ViewMode::Compact => {
            // select_item(.., true) clears the rest, and scroll_to brings the row
            // into view — with virtualization the row may not be realized yet.
            ui.list_selection.select_item(index as u32, true);
            ui.list
                .scroll_to(index as u32, gtk::ListScrollFlags::FOCUS, None);
        }
    }
    update_selection(state, ui);
}

fn open_entry(state: &Rc<RefCell<AppState>>, ui: &Ui, entry: Entry) {
    if let Some(picker) = state.borrow().picker.clone() {
        if !entry.is_dir && picker_entry_can_submit(&picker, &entry) {
            picker_write_and_exit(&picker, vec![entry.path], 0);
        }
        if entry.is_dir {
            navigate(state, entry.path);
        } else {
            ui.status.set_label("Select a supported file to open");
        }
        return;
    }
    if entry.is_dir {
        navigate(state, entry.path);
    } else if std::fs::symlink_metadata(&entry.path).is_err() {
        // A stale row must never hand a vanished path to an external app: some
        // of them create the missing file.
        eprintln!("wf-event: open refused, path is gone: {}", entry.path);
        ui.status.set_label(&format!("{} no longer exists", entry.name));
    } else {
        eprintln!("wf-event: open {}", entry.path);
        match native_core::open_path(&entry.path) {
            Err(err) => ui.status.set_label(&format!("Open failed: {err}")),
            // D-75: only a launch that succeeded clears the tutorial.
            #[cfg(feature = "guide")]
            Ok(()) => guide_note_opened(state, &entry),
            #[cfg(not(feature = "guide"))]
            Ok(()) => {}
        }
    }
}

/// The one New Folder path (background menu, toolbar, options menu, Ctrl+Shift+N):
/// create a real numbered folder, then open the shared inline editor on it with
/// the whole name selected. Nothing is shown or recorded unless the folder exists.
fn create_folder_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let cwd = state.borrow().cwd.clone();
    run_job(
        state,
        ui,
        "Creating folder".to_string(),
        move || {
            let path = native_core::make_numbered_dir(&cwd, "New Folder")?;
            Ok(JobOutcome::undo(
                format!("Created {path}"),
                UndoAction::Created { paths: vec![path] },
            ))
        },
        |state, _, outcome| {
            if let Some(UndoAction::Created { paths }) = &outcome.undo {
                if let Some(path) = paths.first() {
                    let name = Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let mut st = state.borrow_mut();
                    st.inline_rename = Some(InlineRename::new(path, &name));
                    st.reveal_path = Some(path.clone());
                }
            }
        },
    );
}

fn create_file_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let cwd = state.borrow().cwd.clone();
    let ui_for_action = ui.clone();
    show_name_dialog(&ui, "New file", "new.txt", NameSelection::StemOnly, move |name| {
        let state = state.clone();
        let ui = ui_for_action.clone();
        let cwd = cwd.clone();
        run_job(
            state,
            ui,
            format!("Creating file {name}"),
            move || {
                eprintln!("wf-event: new_file {name} in {cwd}");
                let path = native_core::new_file(&cwd, &name)?;
                Ok(JobOutcome::undo(
                    format!("Created {path}"),
                    UndoAction::Created { paths: vec![path] },
                ))
            },
            |_, _, _| {},
        );
    });
}

fn rename_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let entries = selected_entries(&state, &ui);
    if entries.len() > 1 {
        show_batch_rename_dialog(state, ui, entries);
        return;
    }
    let Some(entry) = entries.first().cloned().filter(|_| entries.len() == 1) else {
        ui.status.set_label("Select one item to rename");
        return;
    };
    if f2_starts_inline_rename(entries.len(), state.borrow().view_mode) {
        begin_inline_rename(&state, &ui, &entry);
        return;
    }
    let old_name = entry.name.clone();
    let old_path = entry.path.clone();
    let default_name = old_name.clone();
    let ui_for_action = ui.clone();
    show_name_dialog(
        &ui,
        "Rename",
        &default_name,
        // A folder has no extension to protect, so select all of it.
        if entry.is_dir { NameSelection::All } else { NameSelection::StemOnly },
        move |name| {
        if name == old_name {
            return;
        }
        let state = state.clone();
        let ui = ui_for_action.clone();
        let path = old_path.clone();
        let old_name = old_name.clone();
        run_job(
            state,
            ui,
            format!("Renaming {old_name}"),
            move || {
                let from = path.clone();
                let to = native_core::rename_path(&from, &name)?;
                Ok(JobOutcome::undo(
                    format!("Renamed to {to}"),
                    UndoAction::Renamed { from, to },
                ))
            },
            |_, _, _| {},
        );
    });
}

fn copy_selection(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let paths = selected_paths(state, ui);
    if paths.is_empty() {
        ui.status.set_label("Nothing selected");
        return;
    }
    state.borrow_mut().clipboard = Some(FileClipboard {
        mode: ClipboardMode::Copy,
        paths,
    });
    update_selection(state, ui);
}

fn cut_selection(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let paths = selected_paths(state, ui);
    if paths.is_empty() {
        ui.status.set_label("Nothing selected");
        return;
    }
    state.borrow_mut().clipboard = Some(FileClipboard {
        mode: ClipboardMode::Cut,
        paths,
    });
    update_selection(state, ui);
}

fn paste_clipboard(state: Rc<RefCell<AppState>>, ui: Ui) {
    let Some(clipboard) = state.borrow().clipboard.clone() else {
        ui.status.set_label("Nothing to paste");
        return;
    };
    let cwd = state.borrow().cwd.clone();
    let is_copy = clipboard.mode == ClipboardMode::Copy;
    let clear_after = clipboard.mode == ClipboardMode::Cut;
    transfer_with_preflight(
        state,
        ui,
        Transfer {
            srcs: clipboard.paths,
            dest: cwd,
            is_copy,
            clear_cut_after: clear_after,
            style: TransferStyle::Paste,
        },
    );
}

#[derive(Clone, Copy, PartialEq)]
enum TransferStyle {
    Paste,
    /// Drop on the folder background.
    Drop,
    /// Drop on a folder row; names the destination.
    DropOnRow,
}

#[derive(Clone)]
struct Transfer {
    srcs: Vec<String>,
    dest: String,
    is_copy: bool,
    clear_cut_after: bool,
    style: TransferStyle,
}

/// The one gate for clipboard copy/cut and both drop targets. No clash runs
/// straight away; a clash asks Replace / Duplicate / Cancel once for the whole
/// operation, and Cancel starts nothing.
fn transfer_with_preflight(state: Rc<RefCell<AppState>>, ui: Ui, transfer: Transfer) {
    match plan_transfer(&transfer) {
        TransferPlan::NoOp => ui.status.set_label("Already in this folder"),
        TransferPlan::Run => {
            run_transfer(state, ui, transfer, native_core::PasteConflict::KeepBoth);
        }
        TransferPlan::Ask(conflicts) => {
            let ui_for_choice = ui.clone();
            show_conflict_dialog(&ui, &conflicts, move |mode| {
                run_transfer(state.clone(), ui_for_choice.clone(), transfer.clone(), mode);
            });
        }
    }
}

#[derive(Debug, PartialEq)]
enum TransferPlan {
    /// Every item is already at its destination and this is a MOVE.
    NoOp,
    /// No clash: run straight away.
    Run,
    /// Clashing names: ask once for the whole operation.
    Ask(Vec<String>),
}

fn plan_transfer(transfer: &Transfer) -> TransferPlan {
    if native_core::is_noop_move(&transfer.srcs, &transfer.dest, transfer.is_copy) {
        return TransferPlan::NoOp;
    }
    let conflicts = native_core::check_conflicts(&transfer.srcs, &transfer.dest, transfer.is_copy);
    if conflicts.is_empty() {
        TransferPlan::Run
    } else {
        TransferPlan::Ask(conflicts)
    }
}

/// The dialog seam: returns the mode to run with, or None when nothing may
/// start (Cancel, Escape, close). `start` is only called for Some.
fn apply_conflict_response(response: &str, start: impl FnOnce(native_core::PasteConflict)) {
    if let Some(mode) = native_core::conflict_response(response) {
        start(mode);
    }
}

/// A cut clipboard is emptied only when the job succeeded.
fn clipboard_after_job<T>(clear_cut_after: bool, succeeded: bool, clipboard: Option<T>) -> Option<T> {
    if clear_cut_after && succeeded {
        None
    } else {
        clipboard
    }
}

/// Replace / Duplicate / Cancel. Escape or closing the window is Cancel.
/// Heading and optional detail. One clash: heading only. Several: the names and
/// the one-choice-applies-to-all statement, and no explanation of the buttons.
fn conflict_dialog_text(conflicts: &[String]) -> (String, Option<String>) {
    let count = conflicts.len();
    if count == 1 {
        return (format!("\"{}\" already exists here.", conflicts[0]), None);
    }
    let sample: Vec<&str> = conflicts.iter().take(5).map(String::as_str).collect();
    let more = count.saturating_sub(sample.len());
    let listed = sample.join(", ");
    let names = if more > 0 { format!("{listed}, and {more} more.") } else { listed };
    (
        format!("{count} items already exist here."),
        Some(format!("{names}\n\nYour choice applies to all of them.")),
    )
}

fn show_conflict_dialog<F>(ui: &Ui, conflicts: &[String], on_choice: F)
where
    F: Fn(native_core::PasteConflict) + 'static,
{
    let (heading, detail) = conflict_dialog_text(conflicts);

    let dialog = adw::AlertDialog::new(Some(&heading), detail.as_deref());
    dialog.add_responses(&[
        ("cancel", "Cancel"),
        ("duplicate", "Duplicate"),
        ("replace", "Overwrite"),
    ]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("duplicate"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        apply_conflict_response(response, &on_choice);
    });
    dialog.present(Some(&ui.window));
}

/// The disk work of every transfer: one transaction for the whole batch, checked
/// against the disk before it is committed.
fn perform_transfer(
    srcs: &[String],
    dest: &str,
    is_copy: bool,
    on_conflict: native_core::PasteConflict,
) -> Result<native_core::TransferResult, String> {
    native_core::transfer_batch(srcs, dest, is_copy, on_conflict, &|_| Ok(()), false)
}

/// A committed transfer always yields Undo, even with cleanup warnings; an error
/// means nothing changed and yields none.
fn transfer_outcome(
    ed: &str,
    is_copy: bool,
    result: Result<native_core::TransferResult, String>,
) -> Result<JobOutcome, String> {
    let native_core::TransferResult { items, warnings } = result?;
    let mut message = format!("{ed} {} item{}", items.len(), plural(items.len()));
    if !warnings.is_empty() {
        message.push_str(&format!(" (warning: {})", warnings.join("; ")));
    }
    Ok(JobOutcome::undo(message, UndoAction::Pasted { items, was_copy: is_copy }))
}

/// True when keyboard focus is `root` or somewhere inside it.
fn focus_within(window: &impl IsA<gtk::Window>, root: &impl IsA<gtk::Widget>) -> bool {
    let root = root.upcast_ref::<gtk::Widget>();
    window.focus().is_some_and(|focus| &focus == root || focus.is_ancestor(root))
}

/// When a folder's last row is removed while focus sat on a file row, GTK's
/// next-frame focus search skips the empty view and lands on the selectable
/// preview label, which raises a critical. An empty view holds no rows, so
/// focusing its root keeps focus in the file pane and on a widget that survives.
fn keep_focus_on_empty_view(root: &impl IsA<gtk::Widget>, focus_was_in_view: bool, is_empty: bool) {
    if focus_was_in_view && is_empty {
        // An empty FlowBox is not focusable by default and would pass focus on.
        root.set_focusable(true);
        root.grab_focus();
    }
}

fn run_transfer(
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    transfer: Transfer,
    on_conflict: native_core::PasteConflict,
) {
    let Transfer { srcs, dest, is_copy, clear_cut_after, style } = transfer;
    let count = srcs.len();
    let (ing, ed) = match (style, is_copy) {
        (TransferStyle::Paste, _) => ("Pasting", "Pasted"),
        (_, true) => ("Copying", "Copied"),
        (_, false) => ("Moving", "Moved"),
    };
    let mut label = match style {
        TransferStyle::Paste => format!("{ing} {count}"),
        TransferStyle::Drop => format!("{ing} {count} dropped item{}", plural(count)),
        TransferStyle::DropOnRow => {
            format!("{ing} {count} item{} to {}", plural(count), tab_label(&dest))
        }
    };
    if on_conflict == native_core::PasteConflict::Replace {
        label.push_str(", replacing existing");
    }
    run_job(
        state,
        ui,
        label,
        move || {
            // A failed MOVE is reported as a failure and never retried as COPY.
            transfer_outcome(ed, is_copy, perform_transfer(&srcs, &dest, is_copy, on_conflict))
        },
        move |state, _, _| {
            // after_success only runs when the job succeeded.
            let mut st = state.borrow_mut();
            st.clipboard = clipboard_after_job(clear_cut_after, true, st.clipboard.take());
        },
    );
}

fn trash_selection(state: Rc<RefCell<AppState>>, ui: Ui) {
    let paths = selected_paths(&state, &ui);
    if paths.is_empty() {
        ui.status.set_label("Nothing selected");
        return;
    }
    let count = paths.len();
    run_job(
        state,
        ui,
        format!("Moving {count} item{} to Trash", plural(count)),
        move || {
            native_core::trash_paths(&paths)?;
            Ok(JobOutcome::undo(
                format!("Moved {count} item{} to Trash", plural(count)),
                UndoAction::Trashed { paths },
            ))
        },
        |_, _, _| {},
    );
}

fn permanent_delete_selection(state: Rc<RefCell<AppState>>, ui: Ui) {
    let paths = selected_paths(&state, &ui);
    if paths.is_empty() {
        ui.status.set_label("Nothing selected");
        return;
    }
    let count = paths.len();
    show_confirm_dialog(
        &ui,
        "Delete permanently?",
        &format!(
            "{count} item{} will be deleted without using Trash.",
            plural(count)
        ),
        "Delete",
        {
            let ui_for_action = ui.clone();
            move || {
                let state = state.clone();
                let ui = ui_for_action.clone();
                let paths = paths.clone();
                run_job(
                    state,
                    ui,
                    format!("Deleting {count} item{}", plural(count)),
                    move || {
                        native_core::delete_permanent(&paths)?;
                        Ok(JobOutcome::message(format!(
                            "Deleted {count} item{}",
                            plural(count)
                        )))
                    },
                    |_, _, _| {},
                );
            }
        },
    );
}

fn terminal_here(state: Rc<RefCell<AppState>>, ui: Ui) {
    let cwd = state.borrow().cwd.clone();
    launch_terminal(&ui, &cwd);
}

/// The drive list is refreshed every 30 seconds while the window lives.
const DRIVE_REFRESH_SECS: u32 = 30;

type DriveScan = Arc<dyn Fn() -> Vec<Drive> + Send + Sync>;

/// One refresh now, then one every 30 seconds until the window is gone.
fn start_drive_refresh(state: &Rc<RefCell<AppState>>) {
    request_drive_refresh(state);
    let weak = Rc::downgrade(state);
    glib::timeout_add_seconds_local(DRIVE_REFRESH_SECS, move || {
        let Some(state) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if ui_for_state(&state).is_none() {
            return glib::ControlFlow::Break;
        }
        request_drive_refresh(&state);
        glib::ControlFlow::Continue
    });
}

/// Scans drives and space on a worker and applies the result on the GTK loop.
/// At most one worker runs; a request that arrives meanwhile reruns afterwards
/// so a mount or unmount is never shown from a scan that began before it.
fn request_drive_refresh(state: &Rc<RefCell<AppState>>) {
    {
        let mut state = state.borrow_mut();
        if state.drive_refresh_in_flight {
            state.drive_refresh_again = true;
            return;
        }
        state.drive_refresh_in_flight = true;
    }
    let scan = state.borrow().drive_scan.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(scan());
    });
    let weak = Rc::downgrade(state);
    glib::timeout_add_local(Duration::from_millis(40), move || {
        let Some(state) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let result = match rx.try_recv() {
            Ok(drives) => Some(drives),
            Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => None,
        };
        let again = {
            let mut state = state.borrow_mut();
            state.drive_refresh_in_flight = false;
            std::mem::take(&mut state.drive_refresh_again)
        };
        let Some(ui) = ui_for_state(&state) else {
            return glib::ControlFlow::Break;
        };
        if let Some(drives) = result {
            state.borrow_mut().drives = drives;
            rebuild_sidebar(&state, &ui);
            #[cfg(feature = "guide")]
            guide_sync_crucial(&state, &ui);
        }
        if again {
            request_drive_refresh(&state);
        }
        glib::ControlFlow::Break
    });
}

fn open_drive_action(state: Rc<RefCell<AppState>>, ui: Ui, drive: Drive) {
    if !drive.mountpoint.is_empty() {
        navigate(&state, drive.mountpoint);
        return;
    }

    let device = drive.path.clone();
    run_job(
        state,
        ui,
        format!("Mounting {}", drive.name),
        move || {
            mount_outcome(native_core::mount_drive(&device))
        },
        |state, _, _| request_drive_refresh(state),
    );
}

/// Only a verified mountpoint announces a location and navigates; an error has
/// no `refresh_to`, so the current folder stays. A mount another component won
/// the race for is reported as already mounted.
fn mount_outcome(result: Result<native_core::Mounted, String>) -> Result<JobOutcome, String> {
    Ok(match result? {
        native_core::Mounted::New(path) => JobOutcome::navigate(format!("Mounted at {path}"), path),
        native_core::Mounted::Already(path) => JobOutcome::navigate(format!("Already mounted at {path}"), path),
    })
}

fn unmount_drive_action(state: Rc<RefCell<AppState>>, ui: Ui, drive: Drive) {
    let device = drive.path.clone();
    run_job(
        state,
        ui,
        format!("Unmounting {}", drive.name),
        move || {
            native_core::unmount_drive(&device)?;
            Ok(JobOutcome::message("Unmounted drive"))
        },
        |state, _, _| request_drive_refresh(state),
    );
}

fn eject_drive_action(state: Rc<RefCell<AppState>>, ui: Ui, drive: Drive) {
    let device = drive.path.clone();
    run_job(
        state,
        ui,
        format!("Ejecting {}", drive.name),
        move || {
            native_core::eject_drive(&device)?;
            Ok(JobOutcome::message("Drive is safe to remove"))
        },
        |state, _, _| request_drive_refresh(state),
    );
}

fn recursive_search_action(state: Rc<RefCell<AppState>>, ui: Ui, query: String) {
    let root = state.borrow().cwd.clone();
    let show_hidden = state.borrow().show_hidden;
    let search_in_files = state.borrow().search_in_files;
    ui.status
        .set_label(&format!("Searching {root} for \"{query}\""));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn({
        let query = query.clone();
        move || {
            let _ = tx.send(native_core::search_dir(
                &root,
                &query,
                search_in_files,
                show_hidden,
            ));
        }
    });

    glib::timeout_add_local(Duration::from_millis(150), move || match rx.try_recv() {
        Ok(Ok(entries)) => {
            {
                let mut state = state.borrow_mut();
                state.entries = entries;
                state.visible_entries.clear();
                state.filter_text = query.clone();
            }
            repopulate_rows(&state, &ui);
            update_selection(&state, &ui);
            ui.status.set_label(&format!(
                "Search results for \"{}\"  -  {} shown",
                query,
                state.borrow().visible_entries.len()
            ));
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            ui.status.set_label(&format!("Search failed: {err}"));
            glib::ControlFlow::Break
        }
        Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(TryRecvError::Disconnected) => {
            ui.status.set_label("Search failed");
            glib::ControlFlow::Break
        }
    });
}

fn compress_selection(state: Rc<RefCell<AppState>>, ui: Ui) {
    let paths = selected_paths(&state, &ui);
    if paths.is_empty() {
        ui.status.set_label("Nothing selected");
        return;
    }
    let cwd = state.borrow().cwd.clone();
    let count = paths.len();
    run_job(
        state,
        ui,
        format!("Compressing {count} item{}", plural(count)),
        move || {
            let zip = native_core::compress_zip(&paths, &cwd)?;
            Ok(JobOutcome::undo(
                format!("Created {zip}"),
                UndoAction::Created { paths: vec![zip] },
            ))
        },
        |_, _, _| {},
    );
}

/// Extract into a new folder named after the archive, then select it.
fn extract_archive_to_folder_action(state: Rc<RefCell<AppState>>, ui: Ui, path: String) {
    let cwd = state.borrow().cwd.clone();
    let stem = native_core::archive_stem(&path);
    run_job(
        state,
        ui,
        format!("Extracting to {stem}"),
        move || {
            let created = native_core::extract_archive_into_new_dir(&path, &cwd)?;
            Ok(JobOutcome::message(format!(
                "Extracted to {}",
                Path::new(&created)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or(created)
            )))
        },
        |_, _, _| {},
    );
}

fn extract_archive_action(state: Rc<RefCell<AppState>>, ui: Ui, path: String) {
    let cwd = state.borrow().cwd.clone();
    run_job(
        state,
        ui,
        "Extracting archive".to_string(),
        move || {
            native_core::extract_archive(&path, &cwd)?;
            Ok(JobOutcome::message("Extracted archive"))
        },
        |_, _, _| {},
    );
}

fn empty_trash_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    show_confirm_dialog(
        &ui,
        "Empty Trash?",
        "All items in Trash will be permanently deleted.",
        "Empty Trash",
        {
            let ui_for_action = ui.clone();
            move || {
                run_job(
                    state.clone(),
                    ui_for_action.clone(),
                    "Emptying Trash".to_string(),
                    move || {
                        native_core::empty_trash()?;
                        Ok(JobOutcome::message("Trash emptied"))
                    },
                    |_, _, _| {},
                );
            }
        },
    );
}

fn copy_paths_to_system_clipboard(state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let mut paths = selected_paths(state, ui);
    if paths.is_empty() {
        paths.push(state.borrow().cwd.clone());
    }
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(&paths.join("\n"));
        ui.status.set_label(&format!(
            "Copied {} path{} to clipboard",
            paths.len(),
            plural(paths.len())
        ));
    }
}

fn push_undo(state: &Rc<RefCell<AppState>>, undo: UndoAction) {
    let mut state = state.borrow_mut();
    state.undo_stack.push(undo);
    let overflow = state.undo_stack.len().saturating_sub(100);
    if overflow > 0 {
        state.undo_stack.drain(0..overflow);
    }
}

fn undo_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let Some(action) = state.borrow().undo_stack.last().cloned() else {
        ui.status.set_label("Nothing to undo");
        return;
    };
    let label = undo_label(&action);
    run_job(
        state,
        ui,
        format!("Undoing {label}"),
        move || perform_undo(action),
        |state, _, _| {
            state.borrow_mut().undo_stack.pop();
        },
    );
}

fn undo_label(action: &UndoAction) -> String {
    match action {
        UndoAction::Created { paths } => {
            format!("create/copy of {} item{}", paths.len(), plural(paths.len()))
        }
        UndoAction::Renamed { .. } => "rename".to_string(),
        UndoAction::BatchRenamed { items } => {
            format!(
                "batch rename of {} item{}",
                items.len(),
                plural(items.len())
            )
        }
        UndoAction::Pasted { items, was_copy } => {
            let verb = if *was_copy { "copy" } else { "move" };
            format!("{verb} of {} item{}", items.len(), plural(items.len()))
        }
        UndoAction::Trashed { paths } => {
            format!("Trash move of {} item{}", paths.len(), plural(paths.len()))
        }
    }
}

fn perform_undo(action: UndoAction) -> Result<JobOutcome, String> {
    match action {
        UndoAction::Created { paths } => {
            let count = paths.len();
            native_core::trash_paths(&paths)?;
            Ok(JobOutcome::message(format!(
                "Undid create/copy: moved {count} item{} to Trash",
                plural(count)
            )))
        }
        UndoAction::Renamed { from, to } => {
            let old_name = PathBuf::from(&from)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("bad original name")?
                .to_string();
            native_core::rename_path(&to, &old_name)?;
            Ok(JobOutcome::message("Undid rename"))
        }
        UndoAction::BatchRenamed { items } => {
            let count = items.len();
            native_core::restore_renamed_items(&items)?;
            Ok(JobOutcome::message(format!(
                "Undid batch rename of {count} item{}",
                plural(count)
            )))
        }
        UndoAction::Pasted { items, was_copy } => {
            let count = items.len();
            if was_copy {
                let paths: Vec<String> = items.into_iter().map(|item| item.to).collect();
                native_core::trash_paths(&paths)?;
                Ok(JobOutcome::message(format!(
                    "Undid copy: moved {count} item{} to Trash",
                    plural(count)
                )))
            } else {
                native_core::restore_moved_items(&items)?;
                Ok(JobOutcome::message(format!(
                    "Undid move of {count} item{}",
                    plural(count)
                )))
            }
        }
        UndoAction::Trashed { paths } => {
            let count = paths.len();
            native_core::restore_from_trash(&paths)?;
            Ok(JobOutcome::message(format!(
                "Restored {count} item{} from Trash",
                plural(count)
            )))
        }
    }
}

fn run_job<F, A>(state: Rc<RefCell<AppState>>, ui: Ui, label: String, job: F, after_success: A)
where
    F: FnOnce() -> Result<JobOutcome, String> + Send + 'static,
    A: Fn(&Rc<RefCell<AppState>>, &Ui, &JobOutcome) + 'static,
{
    let refresh_path = state.borrow().cwd.clone();
    ui.status.set_label(&label);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });

    glib::timeout_add_local(Duration::from_millis(150), move || match rx.try_recv() {
        Ok(Ok(outcome)) => {
            after_success(&state, &ui, &outcome);
            if let Some(undo) = outcome.undo.clone() {
                push_undo(&state, undo);
            }
            let target = outcome
                .refresh_to
                .clone()
                .unwrap_or_else(|| refresh_path.clone());
            navigate_to_with_message(
                &state,
                target,
                NavMode::Record,
                Some(outcome.message.clone()),
            );
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            ui.status.set_label(&format!("Operation failed: {err}"));
            glib::ControlFlow::Break
        }
        Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(TryRecvError::Disconnected) => {
            ui.status.set_label("Operation failed");
            glib::ControlFlow::Break
        }
    });
}

/// Inline (in-row) rename state. Keyed by the ORIGINAL full path, never a row
/// index, so virtualised rows rebinding or a re-sort cannot retarget the edit.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InlineRename {
    path: String,
    text: String,
    error: Option<String>,
    /// True from the moment a commit starts until it succeeds or fails, so
    /// Enter followed by the focus loss it causes writes exactly once.
    pending: bool,
}

impl InlineRename {
    fn new(path: &str, text: &str) -> Self {
        Self { path: path.to_string(), text: text.to_string(), error: None, pending: false }
    }

    /// Claims the single commit slot; false if a commit is already running.
    fn begin_commit(&mut self, text: &str) -> bool {
        if self.pending {
            return false;
        }
        self.pending = true;
        self.text = text.to_string();
        true
    }

    /// A failed commit keeps the field open with the text and the reason.
    fn fail(&mut self, reason: String) {
        self.pending = false;
        self.error = Some(reason);
    }
}

#[derive(Clone, Copy)]
enum InlineEvent {
    Enter,
    Escape,
    FocusLoss,
}

/// Enter is the only gesture that writes; Escape and click-away cancel.
fn inline_event_commits(event: InlineEvent) -> bool {
    matches!(event, InlineEvent::Enter)
}

/// F2 only starts an inline edit for exactly one selected item in a list view.
fn f2_starts_inline_rename(selected: usize, view: ViewMode) -> bool {
    selected == 1 && matches!(view, ViewMode::Details | ViewMode::Compact)
}

/// Opening is explicit: GTK reports an activation by list position only, which
/// can name a different file after a drag or a row rebind. An activation opens
/// only when Enter was just pressed, or the last two presses were on that same
/// path close together. A plain click or Ctrl+click never opens.
#[derive(Default)]
struct ActivationGate {
    presses: Vec<(String, std::time::Instant, bool)>,
    enter_at: Option<std::time::Instant>,
}

impl ActivationGate {
    const DOUBLE: Duration = Duration::from_millis(500);
    const ENTER: Duration = Duration::from_millis(400);

    /// `plain` is false when Ctrl, Shift, Alt or Super was held for the press.
    fn press(&mut self, path: &str, now: std::time::Instant, plain: bool) {
        self.presses.push((path.to_string(), now, plain));
        if self.presses.len() > 2 {
            self.presses.remove(0);
        }
    }

    fn enter(&mut self, now: std::time::Instant) {
        self.enter_at = Some(now);
    }

    fn allows(&mut self, path: &str, now: std::time::Instant) -> bool {
        if self.enter_at.take().is_some_and(|at| now.duration_since(at) <= Self::ENTER) {
            return true;
        }
        let double = match self.presses.as_slice() {
            [(a, ta, pa), (b, tb, pb)] => {
                *pa
                    && *pb
                    && a == path
                    && b == path
                    && tb.duration_since(*ta) <= Self::DOUBLE
                    && now.duration_since(*tb) <= Self::DOUBLE
            }
            _ => false,
        };
        if double {
            self.presses.clear();
        }
        double
    }
}

/// Opening a folder rebuilds the file views and destroys the very row or child
/// whose activation signal is still unwinding. Run the open on the next main
/// context turn, after GTK has finished that signal. The caller captures the
/// entry BEFORE deferring, so a later rebind or selection cannot retarget it.
fn defer_after_signal(open: impl FnOnce() + 'static) {
    glib::idle_add_local_once(open);
}

fn connect_gated_activate(
    list: &gtk::ListView,
    gate: Rc<RefCell<ActivationGate>>,
    path_at: Rc<dyn Fn(u32) -> Option<String>>,
    open: Rc<dyn Fn(String)>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let gate = gate.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter) {
                gate.borrow_mut().enter(std::time::Instant::now());
            }
            glib::Propagation::Proceed
        });
    }
    list.add_controller(keys);
    list.connect_activate(move |_, position| {
        let started = std::time::Instant::now();
        let Some(path) = path_at(position) else { return };
        let allowed = gate.borrow_mut().allows(&path, started);
        if allowed {
            open(path.clone());
        }
        eprintln!(
            "wf-event: activate {path} allowed={allowed} took={}ms",
            started.elapsed().as_millis()
        );
    });
}

/// A slow second click renames only when the name was already selected by an
/// earlier click, and is neither the first click nor a double-click (open).
fn slow_click_starts_rename(
    prior_click_on_same_path: Option<Duration>,
    n_press: i32,
    only_this_selected: bool,
) -> bool {
    n_press == 1
        && only_this_selected
        && prior_click_on_same_path
            .is_some_and(|d| d >= Duration::from_millis(500) && d <= Duration::from_millis(2000))
}

#[allow(deprecated)]
/// How much of the pre-filled name starts out selected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameSelection {
    /// Everything, for a brand-new name the user is expected to replace.
    All,
    /// Explorer parity: the base name only, leaving the extension untouched so
    /// typing a new name cannot silently drop or mangle ".png".
    StemOnly,
}

fn build_name_dialog<F>(
    title: &str,
    default_name: &str,
    on_ok: F,
) -> (adw::AlertDialog, gtk::Entry)
where
    F: Fn(String) + 'static,
{
    // adw::AlertDialog renders INSIDE the parent window rather than as its own
    // toplevel. The old gtk::Dialog was a separate window, so a KWin rule that
    // forces Wayfinder windows to a screen position dragged this dialog off
    // with it — and because it is modal, the app looked frozen while waiting on
    // a box the user could not see. An in-window dialog cannot be misplaced.
    let dialog = adw::AlertDialog::new(Some(title), None);
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "OK")]);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);

    let entry = gtk::Entry::builder()
        .text(default_name)
        .hexpand(true)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));

    // One shared handler, guarded so confirming with Enter and confirming with
    // the button can never both fire.
    let on_ok = Rc::new(on_ok);
    let done = Rc::new(std::cell::Cell::new(false));
    let confirm = {
        let on_ok = on_ok.clone();
        let done = done.clone();
        move |value: String| {
            if done.get() {
                return;
            }
            done.set(true);
            let value = value.trim().to_string();
            if !value.is_empty() {
                on_ok(value);
            }
        }
    };
    let confirm = Rc::new(confirm);

    {
        let entry = entry.clone();
        let confirm = confirm.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "ok" {
                confirm(entry.text().to_string());
            }
        });
    }
    {
        // Enter in the field confirms, as it did before.
        let dialog = dialog.clone();
        let confirm = confirm.clone();
        entry.connect_activate(move |entry| {
            confirm(entry.text().to_string());
            dialog.close();
        });
    }

    (dialog, entry)
}

fn show_name_dialog<F>(
    ui: &Ui,
    title: &str,
    default_name: &str,
    selection: NameSelection,
    on_ok: F,
) where
    F: Fn(String) + 'static,
{
    let (dialog, entry) = build_name_dialog(title, default_name, on_ok);
    dialog.present(Some(&ui.window));
    entry.grab_focus();
    // select_region counts CHARACTERS, not bytes, so a name with any non-ASCII
    // in it would select the wrong amount if this used byte offsets.
    let end = match selection {
        NameSelection::All => -1,
        NameSelection::StemOnly => {
            let (stem, ext) = split_stem_ext(default_name);
            if ext.is_empty() {
                -1
            } else {
                stem.chars().count() as i32
            }
        }
    };
    entry.select_region(0, end);
}

#[allow(deprecated)]
fn show_confirm_dialog<F>(ui: &Ui, title: &str, body: &str, confirm: &str, on_confirm: F)
where
    F: Fn() + 'static,
{
    // In-window, for the same reason as show_name_dialog: a separate toplevel
    // can be moved off-screen by a window rule while still holding a modal
    // grab, which reads as the app freezing.
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("confirm", confirm)]);
    // This guards destructive actions, so Cancel is the default and the
    // confirming button is marked destructive.
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });
    dialog.present(Some(&ui.window));
}

#[allow(deprecated)]
fn show_batch_rename_dialog(state: Rc<RefCell<AppState>>, ui: Ui, entries: Vec<Entry>) {
    let dialog = gtk::Dialog::with_buttons(
        Some("Batch rename"),
        Some(&ui.window),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Rename", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(16);
    content.set_margin_end(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);

    let title = gtk::Label::builder()
        .label(&format!(
            "Rename {} item{}",
            entries.len(),
            plural(entries.len())
        ))
        .xalign(0.0)
        .build();
    let find = gtk::Entry::builder()
        .placeholder_text("Find text")
        .hexpand(true)
        .build();
    let replace = gtk::Entry::builder()
        .placeholder_text("Replace with")
        .hexpand(true)
        .build();
    let pattern = gtk::Entry::builder()
        .placeholder_text("Pattern, for example Photo_### or {name}_###")
        .hexpand(true)
        .build();
    let start = gtk::SpinButton::with_range(0.0, 999_999.0, 1.0);
    start.set_value(1.0);

    let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    replace_row.append(&find);
    replace_row.append(&replace);

    let pattern_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    pattern_row.append(&pattern);
    pattern_row.append(&start);

    let preview = gtk::Label::builder()
        .xalign(0.0)
        .selectable(true)
        .wrap(true)
        .build();
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&preview)
        .min_content_height(180)
        .max_content_height(240)
        .hexpand(true)
        .build();

    content.append(&title);
    content.append(&replace_row);
    content.append(&pattern_row);
    content.append(&preview_scroll);
    dialog.content_area().append(&content);

    let preview_entries = entries.clone();
    let preview_for_refresh = preview.clone();
    let find_for_refresh = find.clone();
    let replace_for_refresh = replace.clone();
    let pattern_for_refresh = pattern.clone();
    let start_for_refresh = start.clone();
    let refresh_preview: Rc<dyn Fn()> = Rc::new(move || {
        let requests = batch_rename_requests(
            &preview_entries,
            find_for_refresh.text().as_str(),
            replace_for_refresh.text().as_str(),
            pattern_for_refresh.text().as_str(),
            start_for_refresh.value_as_int(),
        );
        let mut lines: Vec<String> = requests
            .iter()
            .take(12)
            .map(|(_, old_name, new_name)| format!("{old_name} -> {new_name}"))
            .collect();
        if requests.len() > 12 {
            lines.push(format!("...and {} more", requests.len() - 12));
        }
        if lines.is_empty() {
            lines.push("No changes yet".to_string());
        }
        preview_for_refresh.set_label(&lines.join("\n"));
    });

    for entry in [&find, &replace, &pattern] {
        let refresh = refresh_preview.clone();
        entry.connect_changed(move |_| refresh());
    }
    {
        let refresh = refresh_preview.clone();
        start.connect_value_changed(move |_| refresh());
    }
    refresh_preview();

    let entries_for_response = entries.clone();
    let find_for_response = find.clone();
    let replace_for_response = replace.clone();
    let pattern_for_response = pattern.clone();
    let start_for_response = start.clone();
    let state_for_response = state.clone();
    let ui_for_response = ui.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Ok {
            let requests = batch_rename_requests(
                &entries_for_response,
                find_for_response.text().as_str(),
                replace_for_response.text().as_str(),
                pattern_for_response.text().as_str(),
                start_for_response.value_as_int(),
            )
            .into_iter()
            .filter(|(_, old_name, new_name)| old_name != new_name && !new_name.contains('/'))
            .map(|(path, _, new_name)| (path, new_name))
            .collect::<Vec<_>>();

            if requests.is_empty() {
                ui_for_response.status.set_label("No batch rename changes");
            } else {
                let count = requests.len();
                run_job(
                    state_for_response.clone(),
                    ui_for_response.clone(),
                    format!("Renaming {count} item{}", plural(count)),
                    move || {
                        let items = native_core::batch_rename(&requests)?;
                        Ok(JobOutcome::undo(
                            format!("Renamed {} item{}", items.len(), plural(items.len())),
                            UndoAction::BatchRenamed { items },
                        ))
                    },
                    |_, _, _| {},
                );
            }
        }
        dialog.close();
    });
    dialog.present();
    find.grab_focus();
}

fn batch_rename_requests(
    entries: &[Entry],
    find: &str,
    replace: &str,
    pattern: &str,
    start: i32,
) -> Vec<(String, String, String)> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                entry.path.clone(),
                entry.name.clone(),
                batch_new_name(&entry.name, find, replace, pattern, start, index),
            )
        })
        .collect()
}

fn batch_new_name(
    name: &str,
    find: &str,
    replace: &str,
    pattern: &str,
    start: i32,
    index: usize,
) -> String {
    let pattern = pattern.trim();
    if !pattern.is_empty() {
        let (stem, ext) = split_stem_ext(name);
        let number = (start.max(0) as usize) + index;
        let mut value = pattern.to_string();
        if let Some((begin, end)) = first_hash_run(pattern) {
            let width = end - begin;
            value.replace_range(begin..end, &format!("{number:0width$}"));
        } else {
            value.push_str(&number.to_string());
        }
        return format!("{}{}", value.replace("{name}", &stem), ext);
    }

    if !find.is_empty() {
        return name.replace(find, replace);
    }

    name.to_string()
}

fn split_stem_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(index) if index > 0 => (name[..index].to_string(), name[index..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

fn first_hash_run(value: &str) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    let start = bytes.iter().position(|byte| *byte == b'#')?;
    let mut end = start;
    while end < bytes.len() && bytes[end] == b'#' {
        end += 1;
    }
    Some((start, end))
}

fn connect_row_context(
    row: &gtk::Box,
    position: u32,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    entry: Entry,
) {
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let row_for_click = row.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        // Claim the gesture so the ListView's background handler does not also
        // fire and clear the selection out from under the menu.
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if !ui.list_selection.is_selected(position) {
            ui.list_selection.select_item(position, true);
            update_selection(&state, &ui);
        }
        show_item_context_menu(
            &row_for_click,
            state.clone(),
            ui.clone(),
            entry.clone(),
            x,
            y,
        );
    });
    row.add_controller(click);
}

fn connect_background_context(list: &gtk::ListView, state: Rc<RefCell<AppState>>, ui: Ui) {
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let list_for_click = list.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        // A row's own right-click handler claims the gesture first, so reaching
        // here means the click landed on empty space below the rows.
        gesture.set_state(gtk::EventSequenceState::Claimed);
        ui.list_selection.unselect_all();
        update_selection(&state, &ui);
        show_background_context_menu(&list_for_click, state.clone(), ui.clone(), x, y);
    });
    list.add_controller(click);
}

/// Grid has no per-item menu yet; a right-click on empty file-pane space (the
/// scroller, or the grid between its items) opens the same background menu.
fn connect_grid_background_context(
    scroll: &gtk::ScrolledWindow,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
) {
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let scroll_for_click = scroll.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        let on_item = scroll_for_click
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|w| w.ancestor(gtk::FlowBoxChild::static_type()).is_some());
        if on_item {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        ui.grid.unselect_all();
        update_selection(&state, &ui);
        show_background_context_menu(&scroll_for_click, state.clone(), ui.clone(), x, y);
    });
    scroll.add_controller(click);
}

fn connect_drive_context(
    row: &gtk::ListBoxRow,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    drive: Drive,
) {
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let row_for_click = row.clone();
    click.connect_pressed(move |_, _, x, y| {
        show_drive_context_menu(
            &row_for_click,
            state.clone(),
            ui.clone(),
            drive.clone(),
            x,
            y,
        );
    });
    row.add_controller(click);
}

fn show_drive_context_menu(
    row: &gtk::ListBoxRow,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    drive: Drive,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_parent(row);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(4);
    menu.set_margin_end(4);
    menu.set_margin_bottom(4);
    menu.set_margin_start(4);

    {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        let label = if drive.mountpoint.is_empty() {
            "Mount & Open"
        } else {
            "Open"
        };
        let button = context_button(label, &popover, {
            let drive = drive.clone();
            move || open_drive_action(state.clone(), ui.clone(), drive.clone())
        });
        #[cfg(feature = "guide")]
        guide_name_drive(&button, if drive.mountpoint.is_empty() { "remount" } else { "open" }, &drive);
        menu.append(&button);
    }

    if drive.kind == "drive" && !drive.mountpoint.is_empty() {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        let button = context_item("media-eject-symbolic", "Unmount", "", &popover, {
            let drive = drive.clone();
            move || unmount_drive_action(state.clone(), ui.clone(), drive.clone())
        });
        #[cfg(feature = "guide")]
        guide_name_drive(&button, "unmount", &drive);
        menu.append(&button);
    }

    if drive.removable {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        let button = context_item("media-eject-symbolic", "Eject", "", &popover, {
            let drive = drive.clone();
            move || eject_drive_action(state.clone(), ui.clone(), drive.clone())
        });
        #[cfg(feature = "guide")]
        guide_name_drive(&button, "eject", &drive);
        menu.append(&button);
    }

    popover.set_child(Some(&menu));
    unparent_when_closed(&popover);
    #[cfg(feature = "guide")]
    guide_repaint_on_close(&popover, &state);
    popover.popup();
    #[cfg(feature = "guide")]
    guide_resolve(&state);
}

fn show_item_context_menu(
    row: &impl IsA<gtk::Widget>,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    entry: Entry,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_parent(row);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(4);
    menu.set_margin_end(4);
    menu.set_margin_bottom(4);
    menu.set_margin_start(4);

    // Explorer leads with a strip of icon-only buttons for the actions you reach
    // for constantly, then the labelled list underneath.
    {
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        strip.add_css_class("wf-menu-strip");
        strip.set_homogeneous(true);
        let tools: [(&str, &str, Box<dyn Fn()>); 5] = [
            (
                "edit-cut-symbolic",
                "Cut",
                Box::new({
                    let (state, ui) = (state.clone(), ui.clone());
                    move || cut_selection(&state, &ui)
                }),
            ),
            (
                "edit-copy-symbolic",
                "Copy",
                Box::new({
                    let (state, ui) = (state.clone(), ui.clone());
                    move || copy_selection(&state, &ui)
                }),
            ),
            (
                "edit-paste-symbolic",
                "Paste",
                Box::new({
                    let (state, ui) = (state.clone(), ui.clone());
                    move || paste_clipboard(state.clone(), ui.clone())
                }),
            ),
            (
                "document-edit-symbolic",
                "Rename",
                Box::new({
                    let (state, ui) = (state.clone(), ui.clone());
                    move || rename_action(state.clone(), ui.clone())
                }),
            ),
            (
                "user-trash-symbolic",
                "Move to Trash",
                Box::new({
                    let (state, ui) = (state.clone(), ui.clone());
                    move || trash_selection(state.clone(), ui.clone())
                }),
            ),
        ];
        for (icon, tooltip, action) in tools {
            let button = gtk::Button::from_icon_name(icon);
            button.set_tooltip_text(Some(tooltip));
            button.add_css_class("flat");
            button.add_css_class("wf-menu-tool");
            let popover = popover.downgrade();
            button.connect_clicked(move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                action();
            });
            strip.append(&button);
        }
        menu.append(&strip);
        menu.append(&context_separator());
    }

    {
        let state = state.clone();
        let ui = ui.clone();
        let entry = entry.clone();
        menu.append(&context_item("document-open-symbolic", "Open", "", &popover, move || {
            open_entry(&state, &ui, entry.clone())
        }));
    }
    if entry.is_dir {
        let state = state.clone();
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_item("tab-new-symbolic", "Open in New Tab", "", &popover, move || {
            open_path_in_new_tab(&state, &ui, path.clone())
        }));
    }
    if entry.is_dir {
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_item("window-new-symbolic", "Open in New Window", "", &popover, move || {
            open_new_window(&ui, path.clone())
        }));
    }
    if !entry.is_dir {
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_open_with(&popover, ui.clone(), path.clone()));
    }
    if entry.is_dir {
        let favorite = is_favorite(&state.borrow(), &entry.path);
        let state_for_fav = state.clone();
        let ui_for_fav = ui.clone();
        let path = entry.path.clone();
        let label = if favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        };
        menu.append(&context_button(label, &popover, move || {
            if favorite {
                remove_favorite_path(&state_for_fav, &ui_for_fav, path.clone());
            } else {
                add_favorite_path(&state_for_fav, &ui_for_fav, path.clone());
            }
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-cut-symbolic", "Cut", "Ctrl+X", &popover, move || {
            cut_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-copy-symbolic", "Copy", "Ctrl+C", &popover, move || {
            copy_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-copy-symbolic", "Copy Paths", "Ctrl+Shift+C", &popover, move || {
            copy_paths_to_system_clipboard(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-paste-symbolic", "Paste", "Ctrl+V", &popover, move || {
            paste_clipboard(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let label = if selected_entries(&state, &ui).len() > 1 {
            "Batch Rename..."
        } else {
            "Rename"
        };
        menu.append(&context_button(label, &popover, move || {
            rename_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("package-symbolic", "Compress to ZIP", "", &popover, move || {
            compress_selection(state.clone(), ui.clone())
        }));
    }
    if native_core::is_archive_path(&entry.path) {
        {
            // Default action: a new folder named after the archive, so a badly
            // packed archive cannot scatter loose files over the current folder.
            let state = state.clone();
            let ui = ui.clone();
            let path = entry.path.clone();
            let label = format!("Extract to \"{}\"", native_core::archive_stem(&entry.path));
            menu.append(&context_button(&label, &popover, move || {
                extract_archive_to_folder_action(state.clone(), ui.clone(), path.clone())
            }));
        }
        let state = state.clone();
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_item("archive-extract-symbolic", "Extract Here", "", &popover, move || {
            extract_archive_action(state.clone(), ui.clone(), path.clone())
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("user-trash-symbolic", "Move to Trash", "Delete", &popover, move || {
            trash_selection(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-delete-symbolic", "Delete Permanently", "Shift+Delete", &popover, move || {
            permanent_delete_selection(state.clone(), ui.clone())
        }));
    }

    menu.append(&context_separator());
    {
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_item("document-properties-symbolic", "Properties", "Alt+Enter", &popover, move || {
            show_properties_for_path(ui.clone(), path.clone())
        }));
    }
    {
        let ui = ui.clone();
        let terminal_path = if entry.is_dir {
            entry.path.clone()
        } else {
            native_core::parent_dir(&entry.path)
        };
        menu.append(&context_item("utilities-terminal-symbolic", "Open Terminal Here", "", &popover, move || {
            launch_terminal(&ui, &terminal_path);
        }));
    }

    // A popover that cannot fit its monitor is popped down again by GTK on its first
    // layout, so the menu never appears and the window is left grabbed. Start from a safe
    // bound so it can map, then size it to the real menu when it maps: the whole menu when
    // its monitor has room, a scrolling bounded one only when it does not.
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .max_content_height(ITEM_MENU_MAX_HEIGHT)
        .child(&menu)
        .build();
    #[cfg(feature = "guide")]
    {
        guide_name(&scroller, "item-menu-cap-d56");
        guide_name(&menu, "item-menu-full-d60");
    }
    popover.set_child(Some(&scroller));
    // GTK adds its own chrome around the scroller and only measures it once mapped, so the
    // final cap is computed when the popover maps, before its first allocation.
    popover.connect_map({
        let scroller = scroller.clone();
        let menu = menu.clone();
        let window = ui.window.clone();
        #[cfg(feature = "guide")]
        let state = state.clone();
        move |popover| {
            let (_, popover_natural, _, _) = popover.measure(gtk::Orientation::Vertical, -1);
            let (_, scroller_natural, _, _) = scroller.measure(gtk::Orientation::Vertical, -1);
            let (_, content_natural, _, _) = menu.measure(gtk::Orientation::Vertical, -1);
            let chrome = (popover_natural - scroller_natural).max(0);
            let monitor_h = popover
                .native()
                .and_then(|n| n.surface())
                .or_else(|| window.surface())
                .and_then(|surface| gdk::Display::default()?.monitor_at_surface(&surface))
                .map(|m| m.geometry().height());
            let cap = match monitor_h {
                Some(h) => item_menu_content_cap(content_natural, chrome, h),
                None => (ITEM_MENU_MAX_HEIGHT - chrome).max(1),
            };
            scroller.set_max_content_height(cap);
            // The scroller has no allocation yet: bind its pending arrow once it has one.
            #[cfg(feature = "guide")]
            {
                let state = state.clone();
                glib::idle_add_local_once(move || {
                    let waiting = matches!(&state.borrow().guide_pending, GuideRequest::Known { name, .. } if name == "item-menu-cap-d56" || name == "item-menu-full-d60");
                    if waiting && state.borrow().guide_visual.is_none() {
                        guide_resolve(&state);
                    }
                });
            }
        }
    });
    unparent_when_closed(&popover);
    #[cfg(feature = "guide")]
    guide_repaint_on_close(&popover, &state);
    popover.popup();
    #[cfg(feature = "guide")]
    guide_resolve(&state);
}

/// Safe first bound that lets an item menu map before its real size is known.
const ITEM_MENU_MAX_HEIGHT: i32 = 400;

/// Room kept between an item menu and the monitor edges.
const ITEM_MENU_EDGE_ALLOWANCE: i32 = 32;

/// Content height for the item menu's scroller: the whole menu when the outer menu (content
/// plus chrome) fits the monitor with the edge allowance, otherwise what is left after chrome.
fn item_menu_content_cap(content_natural: i32, chrome: i32, monitor_h: i32) -> i32 {
    let room = monitor_h - ITEM_MENU_EDGE_ALLOWANCE;
    if content_natural + chrome <= room {
        content_natural
    } else {
        (room - chrome).max(1)
    }
}

fn show_background_context_menu(
    list: &impl IsA<gtk::Widget>,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_parent(list);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.set_margin_top(4);
    menu.set_margin_end(4);
    menu.set_margin_bottom(4);
    menu.set_margin_start(4);

    {
        let (folder_state, folder_ui) = (state.clone(), ui.clone());
        let (file_state, file_ui) = (state.clone(), ui.clone());
        menu.append(&context_submenu(
            "document-new-symbolic",
            "New",
            &popover,
            vec![
                (
                    "folder-new-symbolic",
                    "Folder",
                    "Ctrl+Shift+N",
                    Rc::new(move || create_folder_action(folder_state.clone(), folder_ui.clone())),
                ),
                (
                    "document-new-symbolic",
                    "File",
                    "",
                    Rc::new(move || create_file_action(file_state.clone(), file_ui.clone())),
                ),
            ],
        ));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("edit-paste-symbolic", "Paste", "Ctrl+V", &popover, move || {
            paste_clipboard(state.clone(), ui.clone())
        }));
    }
    if state.borrow().cwd.contains("/Trash/files") {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("user-trash-full-symbolic", "Empty Trash", "", &popover, move || {
            empty_trash_action(state.clone(), ui.clone())
        }));
    }
    {
        let state_for_fav = state.clone();
        let ui_for_fav = ui.clone();
        let cwd = state.borrow().cwd.clone();
        let favorite = is_favorite(&state.borrow(), &cwd);
        let label = if favorite {
            "Remove Current Folder from Favorites"
        } else {
            "Add Current Folder to Favorites"
        };
        menu.append(&context_button(label, &popover, move || {
            if favorite {
                remove_favorite_path(&state_for_fav, &ui_for_fav, cwd.clone());
            } else {
                add_favorite_path(&state_for_fav, &ui_for_fav, cwd.clone());
            }
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("document-properties-symbolic", "Properties", "Alt+Enter", &popover, move || {
            let cwd = state.borrow().cwd.clone();
            show_properties_for_path(ui.clone(), cwd);
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_item("utilities-terminal-symbolic", "Open Terminal Here", "", &popover, move || {
            terminal_here(state.clone(), ui.clone())
        }));
    }

    popover.set_child(Some(&menu));
    unparent_when_closed(&popover);
    popover.popup();
}

fn context_button<F>(label: &str, popover: &gtk::Popover, action: F) -> gtk::Button
where
    F: Fn() + 'static,
{
    context_item("", label, "", popover, action)
}

/// One menu row, laid out like Explorer's: icon, then label hard left, then the
/// keyboard shortcut greyed and hard right.
///
/// Two details that make it read as a menu rather than a stack of buttons:
/// a plain `Button::with_label` CENTRES its text, and the icon slot is reserved
/// even when a row has no icon, so every label starts on the same left edge.
fn menu_row(icon: &str, label: &str, accel: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);

    let image = gtk::Image::new();
    if !icon.is_empty() {
        image.set_icon_name(Some(icon));
    }
    image.set_pixel_size(16);
    image.set_size_request(16, 16);
    image.add_css_class("wf-menu-icon");
    row.append(&image);

    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    row.append(&text);

    if !accel.is_empty() {
        let hint = gtk::Label::builder().label(accel).xalign(1.0).build();
        hint.add_css_class("wf-menu-accel");
        row.append(&hint);
    }
    row
}

fn context_item<F>(
    icon: &str,
    label: &str,
    accel: &str,
    popover: &gtk::Popover,
    action: F,
) -> gtk::Button
where
    F: Fn() + 'static,
{
    let row = menu_row(icon, label, accel);
    let button = gtk::Button::builder()
        .child(&row)
        .halign(gtk::Align::Fill)
        .build();
    button.add_css_class("flat");
    button.add_css_class("wf-menu-item");
    let popover = popover.downgrade();
    button.connect_clicked(move |_| {
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
        action();
    });
    button
}

/// Every dynamic context popover, root or side, is unparented once it has closed,
/// so nothing keeps the closed tree alive. (Emptying one with `set_child(None)` was
/// tried and aborts other GTK tests.) GTK may keep a closed shell, and a side shell
/// keeps its own rows. It waits for the
/// popover to be fully unmapped, then one more turn: unparenting while GTK is still
/// finishing the close leaks a reference. A side popover still hanging off a button
/// of a closing menu is unparented with it, since one left on a finalised button is
/// leaked. GTK itself may keep the empty closed root shell alive.
fn unparent_when_closed(popover: &gtk::Popover) {
    fn side_popovers(widget: &gtk::Widget, out: &mut Vec<gtk::Popover>) {
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            side_popovers(&current, out);
            if let Ok(side) = current.downcast::<gtk::Popover>() {
                out.push(side);
            }
        }
    }
    popover.connect_unmap(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            let mut sides = Vec::new();
            side_popovers(popover.upcast_ref(), &mut sides);
            for side in sides.iter().filter(|side| side.parent().is_some()) {
                side.unparent();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
    });
}

/// A menu row that opens a second popover to its right (Explorer's `New >`).
/// Choosing an entry closes both popovers, then runs the action.
fn context_submenu(
    icon: &str,
    label: &str,
    popover: &gtk::Popover,
    items: Vec<(&'static str, &'static str, &'static str, Rc<dyn Fn()>)>,
) -> gtk::Button {
    let row = menu_row(icon, label, "");
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    arrow.set_pixel_size(12);
    row.append(&arrow);
    let button = gtk::Button::builder().child(&row).halign(gtk::Align::Fill).build();
    button.add_css_class("flat");
    button.add_css_class("wf-menu-item");
    let parent = popover.downgrade();
    button.connect_clicked(move |button| {
        let side = gtk::Popover::new();
        side.set_has_arrow(false);
        side.set_position(gtk::PositionType::Right);
        side.set_parent(button);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(4);
        menu.set_margin_end(4);
        menu.set_margin_bottom(4);
        menu.set_margin_start(4);
        for (icon, label, accel, action) in &items {
            let parent = parent.clone();
            let action = action.clone();
            menu.append(&context_item(icon, label, accel, &side, move || {
                if let Some(parent) = parent.upgrade() {
                    parent.popdown();
                }
                action();
            }));
        }
        side.set_child(Some(&menu));
        unparent_when_closed(&side);
        side.popup();
    });
    button
}

fn context_separator() -> gtk::Separator {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("wf-menu-sep");
    sep
}

fn open_with_apps_for(path: &str) -> Vec<native_core::AppEntry> {
    open_with_test_apps().unwrap_or_else(|| native_core::open_with_apps(path))
}

/// Everything a child application launch needs, decided before any process is
/// created, so a test can stop at `spawn_child` and see exactly what would run.
#[cfg_attr(not(test), allow(dead_code))] // some fields are read only by the test seam
enum ChildLaunch<'a> {
    /// A desktop application opening one file.
    DesktopApp { app_id: &'a str, desktop_file: &'a str, path: &'a str },
    /// A terminal command line that must already be installed.
    Terminal { candidate: &'a native_core::TerminalCandidate, dir: &'a str },
}

/// The launch context of the window the user acted in. KDE reads the startup
/// and activation data from it, which is what places the child on that window's
/// screen instead of wherever a context-free process lands.
fn child_launch_context(ui: &Ui) -> gdk::AppLaunchContext {
    let context = gtk::prelude::WidgetExt::display(&ui.window).app_launch_context();
    context.set_timestamp(gdk::CURRENT_TIME);
    context
}

fn launch_open_with(ui: &Ui, app_id: &str, path: &str) -> Result<(), String> {
    let desktop_file = open_with_desktop_file(app_id)?;
    spawn_child(ui, ChildLaunch::DesktopApp { app_id, desktop_file: &desktop_file, path })
        .map_err(|err| match err {
            native_core::LaunchError::NotFound => "app not found".to_string(),
            native_core::LaunchError::Failed(err) => err,
        })
}

fn launch_terminal(ui: &Ui, dir: &str) {
    let result = native_core::open_terminal_with(dir, |candidate| {
        spawn_child(ui, ChildLaunch::Terminal { candidate, dir })
    });
    if let Err(err) = result {
        ui.status.set_label(&format!("Terminal failed: {err}"));
    }
}

#[cfg(not(test))]
fn open_with_desktop_file(app_id: &str) -> Result<String, String> {
    native_core::open_with_desktop_file(app_id)
}

/// The one place a child application is started, always with the initiating
/// window's launch context.
#[cfg(not(test))]
fn spawn_child(ui: &Ui, launch: ChildLaunch) -> Result<(), native_core::LaunchError> {
    use native_core::LaunchError;
    let context = child_launch_context(ui);
    let failed = |err: glib::Error| LaunchError::Failed(err.message().to_string());
    match launch {
        ChildLaunch::DesktopApp { desktop_file, path, .. } => {
            let app_id = std::path::Path::new(desktop_file)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let info = gio::AppInfo::all()
                .into_iter()
                .find(|info| info.id().is_some_and(|id| id == app_id))
                .ok_or_else(|| LaunchError::Failed("app not found".into()))?;
            let uri = gio::File::for_path(path).uri();
            info.launch_uris(&[uri.as_str()], Some(&context)).map_err(failed)
        }
        ChildLaunch::Terminal { candidate, .. } => {
            if glib::find_program_in_path(candidate.program).is_none() {
                return Err(LaunchError::NotFound);
            }
            let line = candidate
                .argv
                .iter()
                .map(|arg| glib::shell_quote(arg).to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            let info = gio::AppInfo::create_from_commandline(
                line,
                Some(candidate.program),
                gio::AppInfoCreateFlags::SUPPORTS_STARTUP_NOTIFICATION,
            )
            .map_err(failed)?;
            info.launch(&[], Some(&context)).map_err(failed)
        }
    }
}

/// The `Open With...` row. Opens a compact side popover beside the row and
/// leaves the item menu open behind it, so no window or grab is ever created.
/// Choosing an app closes both menus, then launches it.
fn context_open_with(parent: &gtk::Popover, ui: Ui, path: String) -> gtk::Button {
    let row = menu_row("application-x-executable-symbolic", "Open With...", "");
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    arrow.set_pixel_size(12);
    row.append(&arrow);
    let button = gtk::Button::builder().child(&row).halign(gtk::Align::Fill).build();
    button.add_css_class("flat");
    button.add_css_class("wf-menu-item");
    #[cfg(feature = "guide")]
    guide_name(&button, "open-with-other-app");
    let parent = parent.downgrade();
    button.connect_clicked(move |button| {
        let apps = open_with_apps_for(&path);
        if apps.is_empty() {
            if let Some(parent) = parent.upgrade() {
                parent.popdown();
            }
            ui.status.set_label("No Open With applications found");
            return;
        }
        let side = gtk::Popover::new();
        side.set_has_arrow(false);
        side.set_position(gtk::PositionType::Right);
        side.set_parent(button);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu.set_margin_top(4);
        menu.set_margin_end(4);
        menu.set_margin_bottom(4);
        menu.set_margin_start(4);
        for app in apps {
            let parent = parent.clone();
            let ui = ui.clone();
            let path = path.clone();
            let name = app.name.clone();
            menu.append(&context_item("application-x-executable-symbolic", &name, "", &side, move || {
                if let Some(parent) = parent.upgrade() {
                    parent.popdown();
                }
                match launch_open_with(&ui, &app.id, &path) {
                    Ok(()) => ui.status.set_label(&format!("Opened with {}", app.name)),
                    Err(err) => ui.status.set_label(&format!("Open With failed: {err}")),
                }
            }));
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(ITEM_MENU_MAX_HEIGHT)
            .child(&menu)
            .build();
        side.set_child(Some(&scroller));
        unparent_when_closed(&side);
        side.popup();
    });
    button
}

#[allow(deprecated)]
fn show_properties_for_path(ui: Ui, path: String) {
    let props = match native_core::properties(&path) {
        Ok(props) => props,
        Err(err) => {
            ui.status.set_label(&format!("Properties failed: {err}"));
            return;
        }
    };

    let size_or_items = if props.is_dir {
        if props.items >= 0 {
            format!(
                "Contains: {} item{}",
                props.items,
                plural(props.items as usize)
            )
        } else {
            "Contains: unknown".to_string()
        }
    } else {
        format!(
            "Size: {} ({} bytes)",
            native_core::fmt_size(props.size),
            props.size
        )
    };
    let created = if props.created > 0 {
        format!("\nCreated: {}", native_core::fmt_modified(props.created))
    } else {
        String::new()
    };
    let body = format!(
        "Type: {}\nLocation: {}\n{}\nPermissions: {}\nModified: {}{}",
        props.kind,
        props.path,
        size_or_items,
        props.permissions,
        native_core::fmt_modified(props.modified),
        created
    );

    let dialog = gtk::Dialog::with_buttons(
        Some(&props.name),
        Some(&ui.window),
        gtk::DialogFlags::MODAL,
        &[("Close", gtk::ResponseType::Close)],
    );
    let label = gtk::Label::builder()
        .label(&body)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .margin_top(16)
        .margin_end(16)
        .margin_bottom(16)
        .margin_start(16)
        .build();
    dialog.content_area().append(&label);
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

/// LOCKED 12 Sep 2026 — Zito confirmed this drag behaviour works. Do not change
/// it without his say-so. The test `row_drag_beats_selection_box_locked` fails if
/// any piece below is removed: the grouped capture-phase claim gesture, the
/// half-threshold claim, the strip check in prepare, and the row padding CSS.
///
/// `box_select_edge` is the Size label of a Details row. Pressing in the strip
/// just left of it starts a selection box; pressing anywhere else on the row
/// drags the file. Grid items pass None: the grid has no selection box.
/// The paths a drag carries: the selection captured at the press when the
/// pressed row belongs to it, else only the pressed row.
fn drag_payload(snapshot: Option<Vec<String>>, live: Vec<String>, pressed: &str) -> Vec<String> {
    let selected = snapshot.unwrap_or(live);
    if selected.iter().any(|p| p == pressed) {
        selected
    } else {
        vec![pressed.to_string()]
    }
}

/// The compact drag picture: the item's normal icon and its name, translucent,
/// inside an overlay that also carries the opaque count badge for several items.
fn build_drag_ghost(path: &str, count: usize) -> (gtk::Overlay, gtk::Box) {
    let is_dir = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let image = gtk::Image::from_icon_name(if is_dir { "folder" } else { "text-x-generic" });
    image.set_pixel_size(DRAG_GHOST_ICON);
    let label = gtk::Label::new(Some(&name));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(20);
    label.set_xalign(0.0);
    let ghost = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    ghost.append(&image);
    ghost.append(&label);
    ghost.set_halign(gtk::Align::Start);
    ghost.set_valign(gtk::Align::Start);
    ghost.set_opacity(DRAG_GHOST_OPACITY);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&ghost));
    if count > 1 {
        let badge = gtk::Label::new(Some(&count.to_string()));
        badge.add_css_class("wf-drag-badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        overlay.add_overlay(&badge);
    }
    (overlay, ghost)
}

/// The press point, kept inside the ghost so the ghost never jumps: rounded and
/// clamped to 0..=size-1 on each axis.
fn clamp_hotspot(grab: (f64, f64), width: i32, height: i32) -> (i32, i32) {
    let x = (grab.0.round() as i32).clamp(0, (width - 1).max(0));
    let y = (grab.1.round() as i32).clamp(0, (height - 1).max(0));
    (x, y)
}

fn ghost_hotspot(grab: (f64, f64), ghost: &gtk::Box) -> (i32, i32) {
    let (_, w, _, _) = ghost.measure(gtk::Orientation::Horizontal, -1);
    let (_, h, _, _) = ghost.measure(gtk::Orientation::Vertical, -1);
    clamp_hotspot(grab, w, h)
}

fn build_file_drag_source<W: IsA<gtk::Widget>>(
    widget: &W,
    path: String,
    selected: Rc<dyn Fn() -> Vec<String>>,
    in_box_select_strip: Rc<dyn Fn(f64) -> bool>,
) -> (gtk::DragSource, gtk::GestureClick) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    // Win the race against the list's rubber-band without touching any widget
    // state. Events run capture-phase root->target first, then bubble
    // target->root; the list's rubber-band gesture is on the bubble leg, so a
    // capture-phase DragSource on the row sees the press first and a drag that
    // starts ON a file picks it up instead of painting a selection box.
    //
    // The previous attempt flipped `enable-rubberband` from inside a capture
    // handler. That adds/removes a gesture on the widget WHILE GTK is iterating
    // that widget's gesture list, which segfaulted in gtk_widget_run_controllers.
    // Never mutate a widget's controllers during event dispatch.
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    // The selection as it was at the press, before the ordinary press handling
    // can collapse it to the pressed row. Cleared when the gesture ends.
    let pressed_selection: Rc<RefCell<Option<Vec<String>>>> = Rc::new(RefCell::new(None));
    let snap_out;
    {
        let snap = gtk::GestureClick::new();
        snap.set_button(1);
        snap.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (selected, slot) = (selected.clone(), pressed_selection.clone());
        snap.connect_pressed(move |_, _, _, _| {
            *slot.borrow_mut() = Some(selected());
        });
        let slot = pressed_selection.clone();
        snap.connect_released(move |_, _, _, _| *slot.borrow_mut() = None);
        // No connect_stopped clear: the drag claim stops this gesture before the
        // DragSource's delayed prepare, which must still see the press snapshot.
        widget.add_controller(snap.clone());
        snap_out = Some(snap);
    }
    {
        let slot = pressed_selection.clone();
        drag.connect_drag_end(move |_, _, _| *slot.borrow_mut() = None);
        let slot = pressed_selection.clone();
        drag.connect_drag_cancel(move |_, _, _| {
            *slot.borrow_mut() = None;
            false
        });
    }
    // A compact translucent ghost (icon and name of the dragged item only), held
    // at the point the press was on, so the row's own name stays readable under
    // it. The count badge for several items stays opaque.
    let drag_count = Rc::new(std::cell::Cell::new(1usize));
    let grab = Rc::new(std::cell::Cell::new((0.0f64, 0.0f64)));
    {
        let (count, grab, path) = (drag_count.clone(), grab.clone(), path.clone());
        drag.connect_drag_begin(move |_, drag| {
            let (overlay, ghost) = build_drag_ghost(&path, count.get());
            gtk::DragIcon::for_drag(drag).set_child(Some(&overlay));
            let (hot_x, hot_y) = ghost_hotspot(grab.get(), &ghost);
            drag.set_hotspot(hot_x, hot_y);
        });
    }
    let strip_for_prepare = in_box_select_strip.clone();
    drag.connect_prepare(move |_, x, y| {
        if strip_for_prepare(x) {
            return None;
        }
        grab.set((x, y));
        let snapshot = pressed_selection.borrow_mut().take();
        let paths = drag_payload(snapshot, selected(), &path);
        drag_count.set(paths.len());
        let files: Vec<gio::File> = paths
            .iter()
            .map(|path| gio::File::for_path(PathBuf::from(path)))
            .collect();

        // Advertise BOTH a GdkFileList (for GTK targets) and an explicit
        // text/uri-list (for browsers / Electron like ChatGPT in Chrome, which
        // do not understand GdkFileList).
        let uri_list = native_core::uri_list_for_paths(&paths);
        let file_list = gdk::FileList::from_array(&files);
        let providers = [
            gdk::ContentProvider::for_value(&file_list.to_value()),
            gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from_owned(uri_list.into_bytes()),
            ),
        ];
        Some(gdk::ContentProvider::new_union(&providers))
    });
    widget.add_controller(drag.clone());
    (drag, snap_out.expect("snapshot gesture"))
}

fn connect_file_drag<W: IsA<gtk::Widget>>(
    widget: &W,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    path: String,
    box_select_edge: Option<&gtk::Label>,
) {
    let selected: Rc<dyn Fn() -> Vec<String>> = Rc::new(move || selected_paths(&state, &ui));
    connect_file_drag_with(widget, selected, path, box_select_edge);
}

/// The production drag wiring, with the selection source injected so a headless
/// test can drive the very same controllers over a real ListView.
fn connect_file_drag_with<W: IsA<gtk::Widget>>(
    widget: &W,
    selected: Rc<dyn Fn() -> Vec<String>>,
    path: String,
    box_select_edge: Option<&gtk::Label>,
) -> gtk::DragSource {
    let row: gtk::Widget = widget.clone().upcast();
    let edge = box_select_edge.cloned();
    let in_box_select_strip = Rc::new(move |x: f64| {
        let Some(edge) = edge.as_ref() else {
            return false;
        };
        let Some(bounds) = edge.compute_bounds(&row) else {
            return false;
        };
        // The box spacing before the Size label is the column separator.
        let separator = bounds.x() as f64 - ROW_GAP as f64;
        x >= separator - BOX_SELECT_STRIP_W && x < bounds.x() as f64
    });
    let (drag, _snap) =
        build_file_drag_source(widget, path, selected, in_box_select_strip.clone());

    if box_select_edge.is_none() {
        return drag;
    }
    // Capture phase alone did not win. GtkDragSource refuses to start a drag
    // until 100 ms after the press (MIN_TIME_TO_DND in gtkdragsource.c), while
    // the list's rubber-band claims the press the instant the pointer passes the
    // drag threshold. A quick grab-and-pull crossed the threshold inside those
    // 100 ms, so the box won and the DragSource was cancelled.
    //
    // This gesture claims the press first, at half the threshold, on the capture
    // leg. Claiming cancels the list's bubble-phase rubber-band. It is grouped
    // with the DragSource so the claim keeps the DragSource alive, and the drag
    // starts as soon as its 100 ms are up. No widget state is changed here.
    let claim = gtk::GestureDrag::new();
    claim.set_propagation_phase(gtk::PropagationPhase::Capture);
    let row_for_claim = widget.clone().upcast::<gtk::Widget>();
    claim.connect_drag_update(move |gesture, offset_x, offset_y| {
        let Some((start_x, _)) = gesture.start_point() else {
            return;
        };
        if in_box_select_strip(start_x) {
            return;
        }
        let threshold = row_for_claim.settings().gtk_dnd_drag_threshold() as f64 / 2.0;
        if offset_x.abs() > threshold || offset_y.abs() > threshold {
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    widget.add_controller(claim.clone());
    claim.group_with(&drag);
    drag
}

/// Paths offered by a drag, minus the destination folder itself.
fn dragged_paths(value: &glib::Value, exclude: Option<&str>) -> Vec<String> {
    let Ok(files) = value.get::<gdk::FileList>() else {
        return Vec::new();
    };
    files
        .files()
        .into_iter()
        .filter_map(|file| file.path())
        .map(|path| path.to_string_lossy().to_string())
        .filter(|path| Some(path.as_str()) != exclude)
        .collect()
}

/// One drop transaction: the target it was negotiated for, the destination, the
/// full source snapshot and the negotiated action. Motion stores it, drop
/// consumes it once, leave clears it. The modifiers are read only in motion.
#[derive(Clone, Debug, PartialEq)]
struct DropTxn {
    to_row: bool,
    dest: String,
    srcs: Vec<String>,
    is_copy: bool,
}

impl DropTxn {
    fn negotiate(to_row: bool, dest: &str, srcs: &[String], ctrl: bool, shift: bool) -> Self {
        DropTxn {
            to_row,
            dest: dest.to_string(),
            srcs: srcs.to_vec(),
            is_copy: native_core::drag_is_copy(srcs, dest, ctrl, shift),
        }
    }

    fn action(&self) -> gdk::DragAction {
        if self.is_copy { gdk::DragAction::COPY } else { gdk::DragAction::MOVE }
    }

    fn style(&self) -> TransferStyle {
        if self.to_row { TransferStyle::DropOnRow } else { TransferStyle::Drop }
    }

    fn into_transfer(self) -> Transfer {
        let style = self.style();
        Transfer { srcs: self.srcs, dest: self.dest, is_copy: self.is_copy, clear_cut_after: false, style }
    }
}

/// Consume the stored transaction for a drop. Only the exact tuple negotiated
/// for this target, destination and source set is returned; anything else is a
/// refusal (None), never a guessed default action.
fn take_drop_txn(
    slot: &RefCell<Option<DropTxn>>,
    to_row: bool,
    dest: &str,
    srcs: &[String],
) -> Option<DropTxn> {
    let txn = slot.borrow_mut().take()?;
    (txn.to_row == to_row && txn.dest == dest && txn.srcs == srcs).then_some(txn)
}

fn current_drop_modifiers(target: &gtk::DropTarget) -> (bool, bool) {
    let mods = target
        .current_drop()
        .map(|drop| drop.device())
        .map(|device| device.modifier_state())
        .unwrap_or_else(gdk::ModifierType::empty)
        | target.current_event_state()
        | target
            .widget()
            .and_then(|w| w.display().default_seat())
            .and_then(|seat| seat.keyboard())
            .map(|kb| kb.modifier_state())
            .unwrap_or_else(gdk::ModifierType::empty);
    (
        mods.contains(gdk::ModifierType::CONTROL_MASK),
        mods.contains(gdk::ModifierType::SHIFT_MASK),
    )
}

fn build_drop_target(
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    fixed_dest: Option<String>,
) -> gtk::DropTarget {
    let drop = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    // Preload so the offered paths are known while the pointer is still moving.
    drop.set_preload(true);
    let to_row = fixed_dest.is_some();
    let dest_of = {
        let state = state.clone();
        move || fixed_dest.clone().unwrap_or_else(|| state.borrow().cwd.clone())
    };
    let slot: Rc<RefCell<Option<DropTxn>>> = Rc::new(RefCell::new(None));
    drop.connect_motion({
        let (dest_of, slot) = (dest_of.clone(), slot.clone());
        move |target, _, _| {
            let dest = dest_of();
            let srcs = target
                .value()
                .map(|v| dragged_paths(&v, Some(&dest)))
                .unwrap_or_default();
            if srcs.is_empty() {
                *slot.borrow_mut() = None;
                return gdk::DragAction::COPY;
            }
            let (ctrl, shift) = current_drop_modifiers(target);
            let txn = DropTxn::negotiate(to_row, &dest, &srcs, ctrl, shift);
            let action = txn.action();
            if slot.borrow().as_ref().map(|t| t.is_copy) != Some(txn.is_copy) {
                eprintln!("wf-event: motion to_row={to_row} ctrl={ctrl} shift={shift} copy={}", txn.is_copy);
            }
            *slot.borrow_mut() = Some(txn);
            action
        }
    });
    // Leave clears the stored tuple only after the current event is handled, so
    // a leave delivered just before the drop cannot erase it (and a missing
    // tuple refuses the drop rather than guessing an action).
    drop.connect_leave({
        let slot = slot.clone();
        move |_| {
            let slot = slot.clone();
            glib::idle_add_local_once(move || *slot.borrow_mut() = None);
        }
    });
    drop.connect_drop(move |_, value, _, _| {
        let dest = dest_of();
        let srcs = dragged_paths(value, Some(&dest));
        if srcs.is_empty() {
            slot.borrow_mut().take();
            return false;
        }
        let Some(txn) = take_drop_txn(&slot, to_row, &dest, &srcs) else {
            eprintln!("wf-event: drop REFUSED, no matching negotiated tuple for {dest}");
            ui.status.set_label("Drop refused: the drag action was not settled");
            return false;
        };
        eprintln!(
            "wf-event: drop to_row={} dest={} copy={} srcs={:?}",
            txn.to_row, txn.dest, txn.is_copy, txn.srcs
        );
        transfer_with_preflight(state.clone(), ui.clone(), txn.into_transfer());
        true
    });
    drop
}

fn connect_file_drop<W: IsA<gtk::Widget>>(widget: &W, state: Rc<RefCell<AppState>>, ui: Ui) {
    widget.add_controller(build_drop_target(state, ui, None));
}

fn connect_file_drop_to_dir<W: IsA<gtk::Widget>>(
    widget: &W,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    dest_dir: String,
) {
    widget.add_controller(build_drop_target(state, ui, Some(dest_dir)));
}

fn connect_zoom_controller<W: IsA<gtk::Widget>>(
    widget: &W,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    target: ZoomTarget,
) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    scroll.connect_scroll(move |controller, _, dy| {
        let Some(event) = controller.current_event() else {
            return glib::Propagation::Proceed;
        };
        if !event
            .modifier_state()
            .contains(gdk::ModifierType::CONTROL_MASK)
        {
            return glib::Propagation::Proceed;
        }
        let factor = if dy < 0.0 { 1.12 } else { 0.89 };
        {
            let mut state_ref = state.borrow_mut();
            match target {
                ZoomTarget::Files => {
                    state_ref.icon_zoom = (state_ref.icon_zoom * factor).clamp(0.6, 2.4);
                }
                ZoomTarget::Preview => {
                    state_ref.preview_zoom = (state_ref.preview_zoom * factor).clamp(0.5, 4.0);
                }
            }
        }
        match target {
            ZoomTarget::Files => repopulate_rows(&state, &ui),
            ZoomTarget::Preview => {
                if let Some(entry) = selected_entries(&state, &ui).into_iter().next() {
                    update_preview(&ui, &entry);
                }
            }
        }
        glib::Propagation::Stop
    });
    widget.add_controller(scroll);
}

fn update_preview(ui: &Ui, entry: &Entry) {
    if !ui.preview_scroll.is_visible() {
        return;
    }
    if entry.is_dir {
        set_preview_text(ui, &native_core::entry_summary(entry));
        return;
    }

    if native_core::is_image_path(&entry.path) {
        set_preview_image(ui, &entry.path);
        return;
    }

    // Audio: do NOT load into GtkMediaFile. Selecting a song prerolls a GStreamer
    // playback pipeline that aborts (SIGABRT, multiqueue assertion in
    // libgstplayback while parsing MP3), which crashes the whole app. Show file
    // info instead — safe, no GStreamer. (Open the song in a real player to play.)
    if native_core::is_audio_path(&entry.path) {
        set_preview_text(ui, &native_core::entry_summary(entry));
        return;
    }

    if native_core::is_video_path(&entry.path) {
        set_preview_media(ui, &entry.path);
        return;
    }

    match native_core::read_text_preview(&entry.path) {
        Ok(text) => set_preview_text(ui, &text),
        Err(_) => set_preview_text(ui, &native_core::entry_summary(entry)),
    }
}

fn preview_zoom_for(ui: &Ui) -> f64 {
    state_for_ui(ui)
        .map(|state| state.borrow().preview_zoom)
        .unwrap_or(1.0)
}

fn set_preview_text(ui: &Ui, text: &str) {
    if !ui.preview_scroll.is_visible() {
        return;
    }
    ui.preview_media_controls
        .set_media_stream(None::<&gtk::MediaStream>);
    ui.preview_media_picture
        .set_paintable(None::<&gdk::Paintable>);
    let zoom = preview_zoom_for(ui);
    if text == "Select a file" {
        ui.preview.add_css_class("wf-preview-empty");
        ui.preview.set_xalign(0.5);
        ui.preview.set_yalign(0.0);
        ui.preview.set_margin_top(16);
        ui.preview.set_margin_end(10);
        ui.preview.set_margin_bottom(10);
        ui.preview.set_margin_start(10);
    } else {
        ui.preview.remove_css_class("wf-preview-empty");
        ui.preview.set_xalign(0.0);
        ui.preview.set_yalign(0.0);
        ui.preview.set_margin_top(10);
        ui.preview.set_margin_end(10);
        ui.preview.set_margin_bottom(10);
        ui.preview.set_margin_start(10);
    }
    if zoom > 1.05 {
        ui.preview.add_css_class("wf-preview-zoomed");
    } else {
        ui.preview.remove_css_class("wf-preview-zoomed");
    }
    ui.preview.set_label(text);
    ui.preview_stack.set_visible_child_name("text");
}

fn set_preview_image(ui: &Ui, path: &str) {
    if !ui.preview_scroll.is_visible() {
        return;
    }
    ui.preview_media_controls
        .set_media_stream(None::<&gtk::MediaStream>);
    ui.preview_media_picture
        .set_paintable(None::<&gdk::Paintable>);
    let file = gio::File::for_path(PathBuf::from(path));
    let zoom = preview_zoom_for(ui);
    let size = if zoom <= 1.0 {
        0
    } else {
        (420.0 * zoom).round() as i32
    };
    ui.preview_picture.set_size_request(size, size);
    ui.preview_picture.set_file(Some(&file));
    ui.preview_stack.set_visible_child_name("image");
}

fn set_preview_media(ui: &Ui, path: &str) {
    if !ui.preview_scroll.is_visible() {
        return;
    }
    let file = gio::File::for_path(PathBuf::from(path));
    let media = gtk::MediaFile::for_file(&file);
    let zoom = preview_zoom_for(ui);
    let size = if zoom <= 1.0 {
        0
    } else {
        (420.0 * zoom).round() as i32
    };
    media.set_loop(false);
    ui.preview_media_picture.set_size_request(size, size);
    ui.preview_media_picture.set_paintable(Some(&media));
    ui.preview_media_controls.set_media_stream(Some(&media));
    ui.preview_stack.set_visible_child_name("media");
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(not(test))]
fn open_with_test_apps() -> Option<Vec<native_core::AppEntry>> {
    None
}

thread_local! {
    /// Test-only stand-ins so a test never launches a real application.
    #[cfg(test)]
    static TEST_DRIVE_SCAN: RefCell<Option<DriveScan>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_OPEN_WITH_APPS: RefCell<Option<Vec<native_core::AppEntry>>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_OPEN_WITH_LAUNCHES: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static TEST_CHILD_ATTEMPTS: RefCell<Vec<ChildRecord>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static TEST_CHILD_MISSING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static TEST_CHILD_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The real scan, or in tests a synthetic one; tests never touch a real drive.
#[cfg(not(test))]
fn drive_scan() -> DriveScan {
    Arc::new(native_core::list_drives)
}

#[cfg(test)]
fn drive_scan() -> DriveScan {
    TEST_DRIVE_SCAN
        .with(|scan| scan.borrow().clone())
        .unwrap_or_else(|| Arc::new(Vec::new))
}

#[cfg(test)]
fn open_with_test_apps() -> Option<Vec<native_core::AppEntry>> {
    TEST_OPEN_WITH_APPS.with(|a| a.borrow().clone())
}

/// What one `spawn_child` call would have started, and the launch context it
/// carried. Nothing is started in a test.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChildRecord {
    /// The desktop file, or the terminal program.
    target: String,
    /// The file for Open With, the directory for a terminal.
    subject: String,
    /// The full terminal command line; empty for Open With.
    argv: Vec<String>,
    /// Whether the context was built from the initiating window's display.
    context_from_display: bool,
}

#[cfg(test)]
fn open_with_desktop_file(app_id: &str) -> Result<String, String> {
    if TEST_OPEN_WITH_APPS.with(|a| a.borrow().is_none()) {
        return native_core::open_with_desktop_file(app_id);
    }
    Ok(format!("/test/{app_id}"))
}

#[cfg(test)]
fn spawn_child(ui: &Ui, launch: ChildLaunch) -> Result<(), native_core::LaunchError> {
    use native_core::LaunchError;
    let context = child_launch_context(ui);
    let from_display = context.property::<gdk::Display>("display") == gtk::prelude::WidgetExt::display(&ui.window);
    let record = match launch {
        ChildLaunch::DesktopApp { app_id, desktop_file, path } => {
            TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().push((app_id.to_string(), path.to_string())));
            ChildRecord { target: desktop_file.into(), subject: path.into(), argv: vec![], context_from_display: from_display }
        }
        ChildLaunch::Terminal { candidate, dir } => {
            let record = ChildRecord {
                target: candidate.program.into(),
                subject: dir.into(),
                argv: candidate.argv.clone(),
                context_from_display: from_display,
            };
            if TEST_CHILD_MISSING.with(|m| m.borrow().iter().any(|p| p == candidate.program)) {
                TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().push(record));
                return Err(LaunchError::NotFound);
            }
            if TEST_CHILD_FAILS.with(|f| f.get()) {
                TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().push(record));
                return Err(LaunchError::Failed("test launch failure".into()));
            }
            record
        }
    };
    TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().push(record));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn transfer_seam_cancel_starts_nothing_and_cut_clears_only_on_success() {
        let started = std::cell::Cell::new(0);
        for response in ["cancel", "close", ""] {
            apply_conflict_response(response, |_| started.set(started.get() + 1));
        }
        assert_eq!(started.get(), 0, "Cancel / close / Escape must not start a transfer");
        apply_conflict_response("replace", |m| {
            assert_eq!(m, native_core::PasteConflict::Replace);
            started.set(started.get() + 1);
        });
        apply_conflict_response("duplicate", |m| {
            assert_eq!(m, native_core::PasteConflict::KeepBoth);
            started.set(started.get() + 1);
        });
        assert_eq!(started.get(), 2);
        // cut clipboard: kept on failure or when not a cut, cleared only on success
        assert_eq!(clipboard_after_job(true, false, Some("cut")), Some("cut"));
        assert_eq!(clipboard_after_job(false, true, Some("copy")), Some("copy"));
        assert_eq!(clipboard_after_job(true, true, Some("cut")), None);
    }

    #[test]
    fn drop_transactions_execute_the_negotiated_action_on_disk() {
        use std::os::unix::fs::MetadataExt;
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-drop-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(root.join("Inbox/Target")).unwrap();
        let inbox = root.join("Inbox").to_string_lossy().to_string();
        let target = root.join("Inbox/Target").to_string_lossy().to_string();
        let s = |p: &str| root.join("Inbox").join(p).to_string_lossy().to_string();
        let body = |n: usize| vec![b'x'; n];
        std::fs::write(s("copy-me.txt"), body(11264)).unwrap();
        std::fs::write(s("bg-copy.txt"), body(16384)).unwrap();
        std::fs::write(s("bg-move.txt"), body(15360)).unwrap();
        std::fs::write(s("m1.txt"), body(100)).unwrap();
        std::fs::write(s("m2.txt"), body(200)).unwrap();

        // Motion negotiates Ctrl COPY; the release-time modifier read is empty and
        // is never consulted: drop consumes the stored transaction.
        let slot = RefCell::new(None);
        let srcs = vec![s("copy-me.txt")];
        *slot.borrow_mut() = Some(DropTxn::negotiate(true, &target, &srcs, true, false));
        let txn = take_drop_txn(&slot, true, &target, &srcs).expect("exact tuple");
        assert!(txn.is_copy);
        assert!(slot.borrow().is_none(), "drop consumes the transaction once");
        let ino = std::fs::metadata(s("copy-me.txt")).unwrap().ino();
        let t = txn.into_transfer();
        assert_eq!(plan_transfer(&t), TransferPlan::Run);
        let items = perform_transfer(&t.srcs, &t.dest, t.is_copy, native_core::PasteConflict::KeepBoth).unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(std::fs::metadata(s("copy-me.txt")).unwrap().ino(), ino);
        assert_eq!(std::fs::read(s("copy-me.txt")).unwrap(), body(11264));
        assert_eq!(std::fs::read(format!("{target}/copy-me.txt")).unwrap(), body(11264));

        // Background Ctrl COPY in the same folder is a clash, not a no-op; Duplicate
        // creates "(2)" and leaves the original inode and bytes alone.
        let srcs = vec![s("bg-copy.txt")];
        *slot.borrow_mut() = Some(DropTxn::negotiate(false, &inbox, &srcs, true, false));
        let t = take_drop_txn(&slot, false, &inbox, &srcs).expect("exact tuple").into_transfer();
        assert!(t.is_copy);
        assert_eq!(plan_transfer(&t), TransferPlan::Ask(vec!["bg-copy.txt".to_string()]));
        let ino = std::fs::metadata(s("bg-copy.txt")).unwrap().ino();
        perform_transfer(&t.srcs, &t.dest, t.is_copy, native_core::PasteConflict::KeepBoth).unwrap();
        assert_eq!(std::fs::metadata(s("bg-copy.txt")).unwrap().ino(), ino);
        assert_eq!(std::fs::read(s("bg-copy.txt")).unwrap(), body(16384));
        assert_eq!(std::fs::read(s("bg-copy (2).txt")).unwrap(), body(16384));

        // Unmodified same-folder background MOVE is a true no-op: same inode and
        // bytes, nothing else appears.
        let srcs = vec![s("bg-move.txt")];
        let before = std::fs::read_dir(&inbox).unwrap().count();
        *slot.borrow_mut() = Some(DropTxn::negotiate(false, &inbox, &srcs, false, false));
        let t = take_drop_txn(&slot, false, &inbox, &srcs).expect("exact tuple").into_transfer();
        assert!(!t.is_copy);
        let ino = std::fs::metadata(s("bg-move.txt")).unwrap().ino();
        assert_eq!(plan_transfer(&t), TransferPlan::NoOp);
        assert_eq!(std::fs::metadata(s("bg-move.txt")).unwrap().ino(), ino);
        assert_eq!(std::fs::read(s("bg-move.txt")).unwrap(), body(15360));
        assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), before);

        // Row MOVE of two files: sources gone, one destination each, same bytes.
        let srcs = vec![s("m1.txt"), s("m2.txt")];
        *slot.borrow_mut() = Some(DropTxn::negotiate(true, &target, &srcs, false, false));
        let t = take_drop_txn(&slot, true, &target, &srcs).expect("exact tuple").into_transfer();
        assert!(!t.is_copy);
        perform_transfer(&t.srcs, &t.dest, t.is_copy, native_core::PasteConflict::KeepBoth).unwrap();
        assert!(!std::path::Path::new(&s("m1.txt")).exists());
        assert!(!std::path::Path::new(&s("m2.txt")).exists());
        assert_eq!(std::fs::read(format!("{target}/m1.txt")).unwrap(), body(100));
        assert_eq!(std::fs::read(format!("{target}/m2.txt")).unwrap(), body(200));

        // A transaction negotiated for one target never feeds another.
        let srcs = vec![s("bg-move.txt")];
        *slot.borrow_mut() = Some(DropTxn::negotiate(true, &target, &srcs, true, false));
        assert!(take_drop_txn(&slot, false, &inbox, &srcs).is_none(), "wrong target refuses");
        assert!(slot.borrow().is_none(), "a refused drop clears the state");
        assert!(take_drop_txn(&slot, true, &target, &srcs).is_none(), "missing tuple refuses");
        // refusal starts nothing: both sides stay byte-identical
        assert_eq!(std::fs::read(s("bg-move.txt")).unwrap(), body(15360));
        assert!(!std::path::Path::new(&format!("{target}/bg-move.txt")).exists());
    }

    #[test]
    fn drag_payload_keeps_the_press_time_selection_for_every_selected_row() {
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-multi-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(root.join("Inbox/Target")).unwrap();
        let p = |rel: &str| root.join(rel).to_string_lossy().to_string();
        let (d, e, other) = (p("Inbox/d-two.txt"), p("Inbox/e-two.txt"), p("Inbox/other.txt"));
        std::fs::write(&d, b"D-BIG-13").unwrap();
        std::fs::write(&e, b"E-BIG-14!").unwrap();
        std::fs::write(&other, b"OTHER").unwrap();
        std::fs::write(p("Inbox/Target/d-two.txt"), b"d1").unwrap();
        std::fs::write(p("Inbox/Target/e-two.txt"), b"e1").unwrap();

        // the live selection has already collapsed to the pressed row
        for pressed in [&d, &e] {
            let paths = drag_payload(Some(vec![d.clone(), e.clone()]), vec![pressed.clone()], pressed);
            assert_eq!(paths, vec![d.clone(), e.clone()], "both selected rows are carried");
            let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
            assert_eq!(gdk::FileList::from_array(&files).files().len(), 2);
        }
        // pressing an unselected row drags only that row
        assert_eq!(
            drag_payload(Some(vec![d.clone(), e.clone()]), vec![other.clone()], &other),
            vec![other.clone()]
        );
        // with no snapshot the live selection is used
        assert_eq!(drag_payload(None, vec![d.clone(), e.clone()], &e), vec![d.clone(), e.clone()]);

        // one aggregate prompt, Duplicate creates both "(2)", MOVE consumes both
        let srcs = drag_payload(Some(vec![d.clone(), e.clone()]), vec![e.clone()], &e);
        let t = Transfer {
            srcs,
            dest: p("Inbox/Target"),
            is_copy: false,
            clear_cut_after: false,
            style: TransferStyle::DropOnRow,
        };
        assert_eq!(
            plan_transfer(&t),
            TransferPlan::Ask(vec!["d-two.txt".to_string(), "e-two.txt".to_string()])
        );
        perform_transfer(&t.srcs, &t.dest, false, native_core::PasteConflict::KeepBoth).unwrap();
        assert_eq!(std::fs::read(p("Inbox/Target/d-two (2).txt")).unwrap(), b"D-BIG-13");
        assert_eq!(std::fs::read(p("Inbox/Target/e-two (2).txt")).unwrap(), b"E-BIG-14!");
        assert_eq!(std::fs::read(p("Inbox/Target/d-two.txt")).unwrap(), b"d1");
        assert!(!std::path::Path::new(&d).exists() && !std::path::Path::new(&e).exists());
        assert_eq!(std::fs::read(&other).unwrap(), b"OTHER");
    }

    #[test]
    fn conflict_dialog_wording_is_short() {
        let (h, d) = conflict_dialog_text(&["a.txt".to_string()]);
        assert_eq!(h, "\"a.txt\" already exists here.");
        assert!(d.is_none(), "a single clash shows only the heading");
        let (h, d) = conflict_dialog_text(&["a".to_string(), "b".to_string()]);
        assert_eq!(h, "2 items already exist here.");
        let d = d.unwrap();
        assert!(d.contains("a, b") && d.contains("applies to all"));
        for banned in ["Undo", "overwrites", "Replace", "keeps both"] {
            assert!(!d.contains(banned));
        }
    }

    #[test]
    fn transfer_postcondition_rejects_a_bad_result() {
        let before = vec![native_core::SourceSnapshot {
            path: "/nonexistent-wf/a".into(),
            ino: 1,
            fp: native_core::Fingerprint::File { len: 5, digest: 1 },
        }];
        let items = vec![native_core::ResolvedItem {
            from: "/nonexistent-wf/a".into(),
            to: "/nonexistent-wf/b".into(),
        }];
        assert!(native_core::verify_transfer(&before, &items, true).is_err());
        assert!(native_core::verify_transfer(&before, &items, false).is_err());
        assert!(native_core::snapshot_sources(&["/nonexistent-wf/a".to_string()]).is_err());
    }

    #[test]
    fn fingerprints_see_every_byte_and_every_tree_entry() {
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-fp-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(&root).unwrap();
        let fp = |p: &std::path::Path| native_core::fingerprint(&p.to_string_lossy()).unwrap();

        // a file over 256 MiB with a change in the middle, away from any edge
        let big = root.join("big.bin");
        {
            let f = std::fs::File::create(&big).unwrap();
            f.set_len(260 * 1024 * 1024).unwrap();
        }
        let first = fp(&big);
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new().write(true).open(&big).unwrap();
            f.seek(SeekFrom::Start(77 * 1024 * 1024 + 12345)).unwrap();
            f.write_all(b"X").unwrap();
        }
        assert_ne!(first, fp(&big), "a one-byte change in the middle is seen");
        std::fs::remove_file(&big).unwrap();

        // nested folder: identical trees match; a rename or equal-size edit differs
        let mk = |name: &str, inner: &str, body: &[u8]| {
            let d = root.join(name);
            std::fs::create_dir_all(d.join("sub")).unwrap();
            std::fs::write(d.join("sub").join(inner), body).unwrap();
            std::os::unix::fs::symlink("target-a", d.join("link")).unwrap();
            d
        };
        let a = mk("a", "f.txt", b"12345");
        let same = mk("same", "f.txt", b"12345");
        let renamed = mk("renamed", "g.txt", b"12345");
        let edited = mk("edited", "f.txt", b"12346");
        assert_eq!(fp(&a), fp(&same));
        assert_ne!(fp(&a), fp(&renamed), "a renamed nested entry differs");
        assert_ne!(fp(&a), fp(&edited), "an equal-size nested edit differs");
    }

    #[test]
    fn transfer_postcondition_compares_bytes_not_lengths() {
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-verify-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt").to_string_lossy().to_string();
        let b = root.join("b.txt").to_string_lossy().to_string();
        std::fs::write(&a, b"AAAAAAAA").unwrap();
        let before = native_core::snapshot_sources(&[a.clone()]).unwrap();
        // an equal-length copy with different bytes must fail
        std::fs::write(&b, b"AAAAAAAB").unwrap();
        let items = vec![native_core::ResolvedItem { from: a.clone(), to: b.clone() }];
        assert!(native_core::verify_transfer(&before, &items, true).is_err());
        std::fs::write(&b, b"AAAAAAAA").unwrap();
        assert!(native_core::verify_transfer(&before, &items, true).is_ok());
        // a source changed by an equal-length overwrite fails a COPY check
        std::fs::write(&a, b"BBBBBBBB").unwrap();
        assert!(native_core::verify_transfer(&before, &items, true).is_err());
    }

    #[test]
    fn plan_transfer_noop_run_ask_and_cancel_leaves_bytes() {
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-plan-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("dest")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"SRC").unwrap();
        std::fs::write(root.join("dest/a.txt"), b"DST").unwrap();
        std::fs::write(root.join("src/free.txt"), b"FREE").unwrap();
        let s = |p: &str| root.join(p).to_string_lossy().to_string();
        let mk = |srcs: Vec<String>, dest: &str, is_copy| Transfer {
            srcs,
            dest: dest.to_string(),
            is_copy,
            clear_cut_after: !is_copy,
            style: TransferStyle::Paste,
        };
        assert_eq!(plan_transfer(&mk(vec![s("src/free.txt")], &s("dest"), false)), TransferPlan::Run);
        assert_eq!(
            plan_transfer(&mk(vec![s("src/a.txt"), s("src/free.txt")], &s("dest"), true)),
            TransferPlan::Ask(vec!["a.txt".to_string()])
        );
        // a MOVE onto its own folder is a true no-op; a COPY offers Duplicate
        assert_eq!(plan_transfer(&mk(vec![s("dest/a.txt")], &s("dest"), false)), TransferPlan::NoOp);
        assert_eq!(
            plan_transfer(&mk(vec![s("dest/a.txt")], &s("dest"), true)),
            TransferPlan::Ask(vec!["a.txt".to_string()])
        );
        // planning and Cancel touch nothing
        assert_eq!(std::fs::read(root.join("src/a.txt")).unwrap(), b"SRC");
        assert_eq!(std::fs::read(root.join("dest/a.txt")).unwrap(), b"DST");
        assert_eq!(std::fs::read(root.join("src/free.txt")).unwrap(), b"FREE");
    }

    use super::*;

    fn entry(name: &str, is_dir: bool, modified: i64) -> Entry {
        Entry {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            is_dir,
            size: 0,
            modified,
            is_image: false,
            icon: String::new(),
            metadata_error: None,
        }
    }

    #[test]
    fn folders_sort_first_like_explorer() {
        let mut v = vec![
            entry("afolder", true, 0),
            entry("bfile.txt", false, 0),
            entry("zfolder", true, 0),
            entry("afile.txt", false, 0),
        ];
        sort_entries(&mut v, SortKey::Name, true);
        let dirs: Vec<bool> = v.iter().map(|e| e.is_dir).collect();
        // Explorer parity: all folders first, no file above any folder.
        assert_eq!(
            dirs,
            vec![true, true, false, false],
            "folders must be on top: {:?}",
            v.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        assert_eq!(v[0].name, "afolder");
        assert_eq!(v[2].name, "afile.txt");
    }

    #[test]
    fn folders_stay_first_when_descending() {
        let mut v = vec![
            entry("a.txt", false, 0),
            entry("folder", true, 0),
            entry("b.txt", false, 0),
        ];
        sort_entries(&mut v, SortKey::Name, false);
        assert!(v[0].is_dir, "folder first even descending");
        assert!(!v[1].is_dir && !v[2].is_dir, "files after folders");
    }

    /// Mirrors the selection maths in `show_name_dialog`. -1 means "select all".
    fn rename_selection_end(name: &str, is_dir: bool) -> i32 {
        if is_dir {
            return -1;
        }
        let (stem, ext) = split_stem_ext(name);
        if ext.is_empty() {
            -1
        } else {
            stem.chars().count() as i32
        }
    }

    /// Runs only under a virtual display (see the env guard), so it can never
    /// open a window on the owner's screen.
    /// All GTK tests share one thread (GTK initializes once) and run only under
    /// a virtual display, so they can never open a window on the owner's screen.
    #[test]
    fn gtk_headless_suite() {
        if std::env::var("WAYFINDER_HEADLESS_GTK_TEST").as_deref() != Ok("1") {
            return;
        }
        gtk::init().expect("gtk init on the virtual display");
        let display = gdk::Display::default().expect("display");
        assert!(
            display.type_().name().contains("X11") && !display.name().contains("wayland"),
            "GTK tests must run on the virtual X display, got {}",
            display.name()
        );
        // First: earlier tests leave modal dialogs that would swallow real pointer input.
        real_pointer_drag_carries_the_press_time_selection();
        // Also real pointer input, so it runs before anything can leave a modal behind.
        #[cfg(feature = "guide")]
        guide_ring_and_label_are_drawn_d52();
        #[cfg(feature = "guide")]
        guide_clears_on_use_d53();
        #[cfg(feature = "guide")]
        guide_open_file_item_d75();
        #[cfg(feature = "guide")]
        guide_crucial_back_d77();
        name_dialog_ok_and_enter_invoke_callback_once_cancel_never();
        list_activation_gate_on_real_listview_opens_only_when_allowed();
        inline_editor_click_away_never_writes_and_enter_writes_once();
        drag_ghost_builds_compact_translucent_item();
        emptied_folder_keeps_focus_on_the_view_root_d20();
        new_folder_menu_and_shared_inline_editor_d21();
        metadata_refresh_keeps_row_identity_and_structure_rebuilds_d22();
        sustained_stream_keeps_objects_pointer_and_timing_d22();
        tabs_keep_independent_history_d27();
        show_hidden_toolbar_and_options_stay_in_sync_d28();
        invert_selection_is_the_exact_complement_d29();
        preview_pane_toggle_is_one_safe_route_d30();
        job_result_survives_its_own_refresh_d32();
        folder_activation_waits_for_gtk_to_finish_d33();
        navigation_hands_focus_to_the_view_root_d34();
        clear_recents_is_durable_and_live_d35();
        settings_survive_the_application_id_d65();
        item_right_click_menu_opens_bounded_and_stays_d41();
        open_with_opens_a_side_menu_not_a_dialog_d42();
        drive_space_bars_refresh_off_thread_and_stay_cached_d46();
        context_menu_actions_fire_once_and_leave_no_popover_d54();
        child_apps_launch_with_the_window_context_d57();
        #[cfg(feature = "guide")]
        guide_request_transport_d48();
        #[cfg(feature = "guide")]
        guide_unknown_response_is_atomic_d58();
        #[cfg(feature = "guide")]
        guide_target_names_are_exact_d50();
        #[cfg(feature = "guide")]
        guide_pending_targets_resolve_d51();
    }

    /// D-77: the arrow on the one file-view surface while browsing the synthetic Launch Box SSD
    /// mount tree. Synthetic drives only: the scan hook never reaches a real drive.
    #[cfg(feature = "guide")]
    fn guide_crucial_back_d77() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d77-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let mount = root.join("mount-crucial");
        let evil = root.join("mount-crucial-evil");
        let outside = root.join("outside");
        let gdir = root.join("guide");
        for d in [&home, &mount.join("LaunchBox"), &mount.join("ES-DE"), &evil, &outside, &gdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let fixtures = [mount.join("LaunchBox/game.txt"), mount.join("ES-DE/theme.txt"), evil.join("x.txt"), outside.join("y.txt")];
        for f in &fixtures {
            std::fs::write(f, "fixture").unwrap();
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        let gb: u64 = 1024 * 1024 * 1024;
        let drive = |name: &str, kind: &str, removable: bool, mp: &str| Drive {
            name: name.to_string(),
            path: "synthetic-crucial-scratch-fixture".into(),
            mountpoint: mp.to_string(),
            size: 100 * gb,
            fstype: "ext4".into(),
            kind: kind.into(),
            removable,
            space: None,
        };
        let mp = mount.to_string_lossy().to_string();
        let good = || drive("Launch Box SSD 2TB", "drive", false, &mp);
        let listing = Arc::new(Mutex::new(Vec::<Drive>::new()));
        {
            let listing = listing.clone();
            let scan: DriveScan = Arc::new(move || listing.lock().unwrap().clone());
            TEST_DRIVE_SCAN.with(|s| *s.borrow_mut() = Some(scan));
        }
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d77test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        install_app_css();
        build_ui(&app, Some(outside.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("outside listing", || state.borrow().cwd == outside.to_string_lossy() && !state.borrow().visible_entries.is_empty());
        wait_for("laid out", || ui.files_stack.width() > 0);
        pump();

        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let label = "Your games are back";
        let put = |name: &str, label: &str| {
            let tmp = gdir.join("wayfinder.json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, label) };
            wait_for("guide request", || state.borrow().guide_pending == want);
            pump();
        };
        let tree = || all_widgets(ui.window.upcast_ref());
        let named = |n: &str| -> Vec<gtk::Widget> { tree().into_iter().filter(|w| w.widget_name() == n).collect() };
        let label_pops = || -> Vec<gtk::Popover> {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-label")).collect()
        };
        let geometry = || (ui.window.width(), ui.window.height(), ui.sidebar.width(), ui.files_stack.width(), ui.files_stack.height(), ui.file_preview.position());
        let before = geometry();
        let refresh = |list: Vec<Drive>| {
            *listing.lock().unwrap() = list;
            request_drive_refresh(&state);
            wait_for("synthetic scan lands", || !state.borrow().drive_refresh_in_flight);
            pump();
        };
        let nav = |path: &std::path::Path| {
            navigate(&state, path.to_string_lossy().to_string());
            wait_for("navigated", || state.borrow().cwd == path.to_string_lossy() && !state.borrow().visible_entries.is_empty());
            pump();
        };
        let none = |why: &str| {
            pump();
            assert!(guide_marked(tree()).is_empty() && label_pops().is_empty(), "{why}: a visual is drawn");
            assert!(named(GUIDE_CRUCIAL_BACK).is_empty(), "{why}: the surface is named");
            assert_eq!(state.borrow().guide_pending, known(GUIDE_CRUCIAL_BACK, label), "{why}: the request stays pending");
        };
        let one = |why: &str| {
            wait_for(why, || guide_marked(tree()).len() == 1 && label_pops().len() == 1);
            let marked = guide_marked(tree());
            let names = named(GUIDE_CRUCIAL_BACK);
            assert_eq!((marked.len(), names.len()), (1, 1), "{why}");
            assert_eq!(marked[0], names[0], "{why}: the named surface is the marked one");
            assert_eq!(marked[0], ui.files_stack.clone().upcast::<gtk::Widget>(), "{why}: the marked surface is the file view");
            assert!(ui.files_stack.is_mapped());
            let pops = label_pops();
            assert!(pops[0].has_css_class("wf-guide-arrow") && !pops[0].can_target() && !pops[0].can_focus(), "{why}");
            let shown = all_widgets(pops[0].upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.label().to_string()));
            assert_eq!(shown.as_deref(), Some(label), "{why}: exact label");
            assert!(tree().into_iter().all(|w| w.widget_name() != GUIDE_CRUCIAL_BACK || w == names[0]), "{why}");
            assert_eq!(geometry(), before, "{why}: geometry moved");
            let focus_in_label = gtk::prelude::GtkWindowExt::focus(&ui.window).is_some_and(|f| f.ancestor(gtk::Popover::static_type()).is_some_and(|p| p.has_css_class("wf-guide-label")));
            assert!(!focus_in_label, "{why}: the arrow took focus");
        };

        // Pending before any scan result, and outside the mount tree.
        put(GUIDE_CRUCIAL_BACK, label);
        none("pending before the scan");
        refresh(vec![good()]);
        none("outside the mount tree");

        // Inside: the root and both folders, through real navigation and Back.
        nav(&mount);
        one("mount root");
        nav(&mount.join("LaunchBox"));
        assert!(state.borrow().visible_entries.iter().any(|e| e.name == "game.txt"), "LaunchBox listing is nonempty");
        one("LaunchBox");
        history_step(&state, false);
        wait_for("back to the root", || state.borrow().cwd == mp);
        one("back at the root");
        nav(&mount.join("ES-DE"));
        assert!(state.borrow().visible_entries.iter().any(|e| e.name == "theme.txt"), "ES-DE listing is nonempty");
        one("ES-DE");
        history_step(&state, false);
        wait_for("back to the root again", || state.borrow().cwd == mp);
        one("back again");
        set_view_mode(&state, &ui, ViewMode::Grid);
        one("grid view");
        set_view_mode(&state, &ui, ViewMode::Compact);
        one("compact view");
        set_view_mode(&state, &ui, ViewMode::Details);
        one("details view");

        // Leaving the tree removes only the visual; the request stays pending.
        nav(&outside);
        none("outside again");
        nav(&evil);
        none("sibling folder that only shares the name prefix");
        nav(&mount);
        one("returned to the mount");

        // The drive goes away and returns.
        refresh(vec![drive("Launch Box SSD 2TB", "drive", false, "")]);
        none("unmounted");
        refresh(vec![good()]);
        one("mounted again");
        refresh(vec![]);
        none("drive removed");
        refresh(vec![good()]);
        one("drive back");

        // Wrong or duplicate contexts fail closed.
        for (why, list) in [
            ("removable", vec![drive("Launch Box SSD 2TB", "drive", true, &mp)]),
            ("wrong case", vec![drive("launch box ssd 2tb", "drive", false, &mp)]),
            ("wrong kind", vec![drive("Launch Box SSD 2TB", "network", false, &mp)]),
            ("relative mountpoint", vec![drive("Launch Box SSD 2TB", "drive", false, "mount-crucial")]),
            ("duplicate eligible", vec![good(), good()]),
        ] {
            refresh(list);
            none(why);
            refresh(vec![good()]);
            one(&format!("restored after {why}"));
        }
        assert!(guide_crucial_context(&[good()], &format!("{mp}/LaunchBox")) && !guide_crucial_context(&[good()], &format!("{mp}-evil")));
        assert!(!guide_crucial_context(&[good()], &format!("{mp}/../outside")), "a parent-traversal cwd is rejected");
        assert!(!guide_crucial_context(&[drive("Launch Box SSD 2TB", "drive", false, &format!("{mp}/../mount-crucial"))], &mp), "a non-normal mountpoint is rejected");

        // Another tutorial's visual survives the same navigation and refresh paths.
        put("terminal-here-folder", "Terminal");
        let circle = guide_marked(tree());
        assert_eq!(circle.len(), 1);
        for step in 0..3 {
            match step {
                0 => nav(&outside),
                1 => refresh(vec![]),
                _ => {
                    refresh(vec![good()]);
                    nav(&mount);
                }
            }
            assert_eq!(guide_marked(tree()), circle, "step {step} disturbed another tutorial");
            assert_eq!(state.borrow().guide_pending, known("terminal-here-folder", "Terminal"));
        }
        put(GUIDE_CRUCIAL_BACK, label);
        one("back to this request");
        put("", "");
        assert!(guide_marked(tree()).is_empty() && label_pops().is_empty(), "clear left a visual");
        for f in &fixtures {
            assert_eq!(std::fs::read_to_string(f).unwrap(), "fixture", "a fixture changed");
        }

        ui.window.destroy();
        TEST_DRIVE_SCAN.with(|s| *s.borrow_mut() = None);
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
    }

    /// D-75: the tutorial circle on the one real `Throwaways/open-me.txt` item, in list and grid
    /// views, through navigation, filtering and view switches, and its clear-on-open.
    #[cfg(feature = "guide")]
    fn guide_open_file_item_d75() {
        use std::os::unix::fs::PermissionsExt;
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let old_path = std::env::var("PATH").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d75-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let throwaways = root.join("Throwaways");
        let elsewhere = root.join("Elsewhere");
        let gdir = root.join("guide");
        let bin = root.join("bin");
        for d in [&home, &throwaways, &elsewhere, &gdir, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(throwaways.join("open-me.txt"), "THROWAWAY OPEN ME").unwrap();
        std::fs::write(throwaways.join("other.txt"), "other").unwrap();
        std::fs::write(elsewhere.join("open-me.txt"), "same name, wrong parent").unwrap();
        // A recording xdg-open: the unchanged open route runs it, and the log counts the opens.
        let log = root.join("opens.log");
        let fake = bin.join("xdg-open");
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$1\" >> '{}'\n", log.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), old_path.clone().unwrap_or_default()));
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d75test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(root.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.set_title(Some("wf-d75-proof"));
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() >= 2);
        pump();

        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let pending = || state.borrow().guide_pending.clone();
        let put = |name: &str, label: &str| {
            let tmp = gdir.join("wayfinder.json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, label) };
            wait_for("guide request", || pending() == want);
            pump();
        };
        let tree = || all_widgets(ui.window.upcast_ref());
        let rings = || -> Vec<gtk::Widget> { guide_marked(tree()) };
        let label_pops = || -> Vec<gtk::Popover> {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-label")).collect()
        };
        let label_text = |p: &gtk::Popover| -> String {
            all_widgets(p.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.label().to_string())).expect("label text")
        };
        let by_name = |n: &str| -> Vec<gtk::Widget> { tree().into_iter().filter(|w| w.widget_name() == n).collect() };
        let no_visual = |why: &str| {
            assert!(rings().is_empty(), "{why}: a circle is still drawn");
            assert!(label_pops().is_empty(), "{why}: a label is still drawn");
            assert!(state.borrow().guide_visual.is_none(), "{why}: visual state dropped");
        };
        let opens = || -> Vec<String> { std::fs::read_to_string(&log).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default() };
        let open_me = throwaways.join("open-me.txt").to_string_lossy().to_string();
        let other = throwaways.join("other.txt").to_string_lossy().to_string();
        let xdo = |args: &[&str]| {
            let out = std::process::Command::new("/usr/bin/xdotool").args(args).output().expect("xdotool");
            assert!(out.status.success(), "xdotool {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let geo = xdo(&["search", "--name", "wf-d75-proof", "getwindowgeometry", "--shell"]);
        let num = |key: &str| -> i32 { geo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse().ok()).expect(key) };
        let (wx, wy) = (num("X="), num("Y="));
        let centre = |w: &gtk::Widget| -> (i32, i32) {
            let b = w.compute_bounds(&ui.window).expect("bounds");
            (wx + (b.x() + b.width() / 2.0) as i32, wy + (b.y() + b.height() / 2.0) as i32)
        };
        let click_at = |w: &gtk::Widget, n: u32| {
            pump();
            let (x, y) = centre(w);
            xdo(&["mousemove", &x.to_string(), &y.to_string()]);
            pump();
            xdo(&["click", "--repeat", &n.to_string(), "--delay", "80", "1"]);
            pump();
            std::thread::sleep(Duration::from_millis(300));
            pump();
        };
        // The item widget for a path: the list row content in list views, the FlowBoxChild in grid.
        let item_for = |path: &str| -> gtk::Widget {
            let mode = state.borrow().view_mode;
            if mode == ViewMode::Grid {
                let entry_ix = state.borrow().visible_entries.iter().position(|e| e.path == path).expect("entry");
                ui.grid.child_at_index(entry_ix as i32).expect("grid child").upcast()
            } else {
                tree().into_iter().find(|w| w.has_css_class("wf-file-row") && w.tooltip_text().as_deref() == Some(path)).expect("list row")
            }
        };
        let geometry = |w: &gtk::Widget| {
            (
                w.compute_bounds(&ui.window).map(|b| (b.x(), b.y(), b.width(), b.height())),
                (w.width(), w.height(), w.is_focusable(), w.can_target(), w.is_sensitive()),
                (ui.window.width(), ui.window.height()),
                GtkWindowExt::focus(&ui.window),
            )
        };

        // Pending before Throwaways is open: no target, no visual.
        put(GUIDE_OPEN_FILE, "Double-click this file");
        no_visual("pending before navigation");
        assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Double-click this file"));
        assert!(by_name(GUIDE_OPEN_FILE).is_empty(), "no item is named before Throwaways opens");

        // Navigate: exactly one real mapped row, one circle, exact label.
        navigate(&state, throwaways.to_string_lossy().to_string());
        wait_for("Throwaways listing", || state.borrow().visible_entries.len() == 2 && by_name(GUIDE_OPEN_FILE).len() == 1);
        wait_for("the circle", || rings().len() == 1 && label_pops().len() == 1);
        let target = by_name(GUIDE_OPEN_FILE).pop().unwrap();
        assert!(target.is_mapped() && target.has_css_class("wf-file-row"), "the real list row content");
        assert_eq!(target.tooltip_text().as_deref(), Some(open_me.as_str()), "the exact fixture path");
        assert_eq!(rings(), vec![target.clone()]);
        let pops = label_pops();
        assert_eq!(pops.len(), 1);
        assert_eq!(label_text(&pops[0]), "Double-click this file");
        assert_eq!(pops[0].parent().as_ref(), Some(&target));
        assert!(!pops[0].can_target() && !pops[0].is_focusable() && !pops[0].can_focus(), "input-transparent, no focus");
        assert!(by_name(GUIDE_OPEN_FILE).iter().all(|w| !w.is::<gtk::FlowBoxChild>()), "no hidden grid duplicate");
        assert!(ui.grid.first_child().is_none(), "the grid holds nothing in list view");
        let shape = geometry(&target);
        drop(pops);

        // A single click only selects: no open, no clear, nothing moved.
        click_at(&target, 1);
        assert_eq!(selected_paths(&state, &ui), vec![open_me.clone()], "single click selects");
        assert!(opens().is_empty(), "single click opens nothing");
        assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Double-click this file"));
        assert_eq!(rings(), vec![target.clone()]);
        assert_eq!(geometry(&target).1, shape.1, "size and focus flags unchanged");

        // Opening a different item leaves the request alone.
        let wrong = item_for(&other);
        click_at(&wrong, 2);
        wait_for("the other file to open", || opens() == vec![other.clone()]);
        assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Double-click this file"), "wrong item does not clear");
        assert_eq!(rings().len(), 1, "circle stays");
        drop(wrong);

        // The real double-click on the exact item: the unchanged route opens once, then clears.
        let target = by_name(GUIDE_OPEN_FILE).pop().unwrap();
        click_at(&target, 2);
        wait_for("open-me.txt to open", || opens().len() == 2);
        pump();
        assert_eq!(opens(), vec![other.clone(), open_me.clone()], "exactly one open of open-me.txt");
        assert_eq!(pending(), GuideRequest::Clear, "cleared by the open");
        assert!(state.borrow().guide_target.is_none());
        no_visual("after the open");
        drop(target);

        // Same request again: rebinds at once; leaving the folder removes the stale visual.
        put(GUIDE_OPEN_FILE, "Again");
        assert_eq!(rings().len(), 1);
        navigate(&state, elsewhere.to_string_lossy().to_string());
        wait_for("Elsewhere listing", || state.borrow().cwd == elsewhere.to_string_lossy());
        wait_for("stale visual gone", || rings().is_empty() && label_pops().is_empty());
        assert!(by_name(GUIDE_OPEN_FILE).is_empty(), "an open-me.txt outside Throwaways is never named");
        assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Again"), "still pending");
        assert!(state.borrow().guide_visual.is_none());

        // Return: one fresh target.
        navigate(&state, throwaways.to_string_lossy().to_string());
        wait_for("back in Throwaways", || by_name(GUIDE_OPEN_FILE).len() == 1 && rings().len() == 1 && label_pops().len() == 1);

        // Filter: the item unbinds, the visual goes; clearing the filter reacquires exactly one.
        state.borrow_mut().filter_text = "other".to_string();
        repopulate_rows(&state, &ui);
        wait_for("filtered out", || by_name(GUIDE_OPEN_FILE).is_empty() && rings().is_empty() && label_pops().is_empty());
        state.borrow_mut().filter_text = String::new();
        repopulate_rows(&state, &ui);
        wait_for("filter cleared", || by_name(GUIDE_OPEN_FILE).len() == 1 && rings().len() == 1 && label_pops().len() == 1);

        // Grid: the FlowBoxChild is the one target, with no list duplicate.
        set_view_mode(&state, &ui, ViewMode::Grid);
        wait_for("grid target", || {
            let named = by_name(GUIDE_OPEN_FILE);
            named.len() == 1 && named[0].is::<gtk::FlowBoxChild>() && rings().len() == 1 && label_pops().len() == 1
        });
        let child = by_name(GUIDE_OPEN_FILE).pop().unwrap();
        assert_eq!(rings(), vec![child.clone()]);
        assert!(child.is_mapped());
        assert_eq!(label_text(&label_pops()[0]), "Again");
        assert!(label_pops()[0].parent().as_ref() == Some(&child));
        // Grid: drawing and a single selecting click change nothing but the selection.
        let shape = geometry(&child);
        click_at(&child, 1);
        assert_eq!(selected_paths(&state, &ui), vec![open_me.clone()], "grid: single click selects");
        let n_grid = opens().len();
        pump();
        assert_eq!(opens().len(), n_grid, "grid: a single click opens nothing");
        assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Again"), "grid: a single click clears nothing");
        assert_eq!(rings(), vec![child.clone()]);
        assert_eq!(geometry(&child).1, shape.1, "grid: size and focus flags unchanged");
        drop(child);

        // Switching back removes the grid target and reacquires one fresh list row.
        set_view_mode(&state, &ui, ViewMode::Details);
        wait_for("list target again", || {
            let named = by_name(GUIDE_OPEN_FILE);
            named.len() == 1 && named[0].has_css_class("wf-file-row") && rings().len() == 1 && label_pops().len() == 1
        });

        // Fail closed: a second widget carrying the name binds nothing.
        guide_name(&ui.status, GUIDE_OPEN_FILE);
        guide_resolve(&state);
        no_visual("duplicate name");
        ui.status.set_widget_name("");
        guide_resolve(&state);
        assert_eq!(rings().len(), 1, "one target again once the duplicate is gone");

        // Negatives at the external-open success boundary: none of these opens anything, so none
        // may clear. Each calls the real open route directly.
        let entry = state.borrow().visible_entries.iter().find(|e| e.path == open_me).cloned().expect("fixture entry");
        let still_pending = |why: &str| {
            assert_eq!(pending(), known(GUIDE_OPEN_FILE, "Back"), "{why}: request kept");
            assert_eq!(rings().len(), 1, "{why}: circle kept");
            assert_eq!(label_pops().len(), 1, "{why}: label kept");
        };
        put(GUIDE_OPEN_FILE, "Back");
        let n = opens().len();
        // A vanished file is refused before any launch.
        let gone = throwaways.join("open-me.txt.gone");
        std::fs::rename(&open_me, &gone).unwrap();
        open_entry(&state, &ui, entry.clone());
        std::fs::rename(&gone, &open_me).unwrap();
        pump();
        assert_eq!(opens().len(), n, "vanished: nothing launched");
        still_pending("vanished file");
        // A launcher that fails (no xdg-open on PATH) reports and leaves the tutorial.
        std::env::set_var("PATH", "");
        open_entry(&state, &ui, entry.clone());
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
        assert!(ui.status.label().contains("Open failed"), "failed launch reported: {}", ui.status.label());
        still_pending("failed open_path");
        // Picker mode (directory picker, so a file is refused and nothing exits).
        state.borrow_mut().picker = Some(PickerState {
            config: PickerConfig { multiple: false, directory: true, starting_dir: None, filter: None, window: None },
            out_file: root.join("picker.out").to_string_lossy().to_string(),
        });
        open_entry(&state, &ui, entry.clone());
        state.borrow_mut().picker = None;
        assert_eq!(opens().len(), n, "picker: nothing launched");
        still_pending("picker mode");
        // A directory never clears it.
        let dir_entry = Entry { is_dir: true, ..entry.clone() };
        assert!(!guide_is_open_file_fixture(&dir_entry));
        guide_note_opened(&state, &dir_entry);
        still_pending("directory entry");
        // The successful, unchanged open route is the only thing that clears it (proved above and here).
        open_entry(&state, &ui, entry.clone());
        wait_for("the successful open", || opens().len() == n + 1);
        assert_eq!(pending(), GuideRequest::Clear, "cleared only after open_path returned Ok");
        no_visual("after the successful open");

        // Unbinding the fixture row while another tutorial's target is painted leaves that alone.
        put("terminal-here-folder", "Other tutorial");
        assert_eq!(rings().len(), 1, "the toolbar circle is painted");
        assert!(by_name(GUIDE_OPEN_FILE).len() == 1, "the fixture row is still bound");
        state.borrow_mut().filter_text = "other".to_string();
        repopulate_rows(&state, &ui);
        wait_for("fixture row unbound", || by_name(GUIDE_OPEN_FILE).is_empty());
        pump();
        assert_eq!(pending(), known("terminal-here-folder", "Other tutorial"), "other request untouched");
        assert_eq!(rings().len(), 1, "the other tutorial's circle survives the unbind");
        assert_eq!(label_pops().len(), 1);
        state.borrow_mut().filter_text = String::new();
        repopulate_rows(&state, &ui);
        pump();
        assert_eq!(rings().len(), 1, "and survives the rebind");

        // Clearing removes it; the request never touched the file.
        put("", "");
        no_visual("cleared");
        assert_eq!(std::fs::read_to_string(&throwaways.join("open-me.txt")).unwrap(), "THROWAWAY OPEN ME");

        ui.window.close();
        pump();
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }

    /// The marked targets: the ones carrying the circle class, and the parent of every
    /// arrow surface. Also fails if the retired ring class is anywhere in the tree.
    #[cfg(feature = "guide")]
    fn guide_marked(tree: Vec<gtk::Widget>) -> Vec<gtk::Widget> {
        assert!(tree.iter().all(|w| !w.has_css_class("wf-guide-ring")), "the retired ring class is on a widget");
        let mut out: Vec<gtk::Widget> = tree.iter().filter(|w| w.has_css_class("wf-guide-circle")).cloned().collect();
        for w in &tree {
            if let Some(p) = w.downcast_ref::<gtk::Popover>() {
                if p.has_css_class("wf-guide-arrow") {
                    out.extend(p.parent());
                }
            }
        }
        out
    }

    #[cfg(feature = "guide")]
    fn guide_ring_and_label_are_drawn_d52() {
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        // D-57: the launch seam succeeds by default; these proofs need the failing handler.
        TEST_CHILD_FAILS.with(|f| f.set(true));
        let root = crate::native_core::scratch_root().join(format!("wf-d52-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let gdir = root.join("guide");
        for d in [&home, &dir, &gdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d52test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.set_title(Some("wf-d52-proof"));
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let frames = std::env::var("WF_D52_FRAMES").ok().map(std::path::PathBuf::from);
        let frame = |name: &str| {
            if let Some(dir) = &frames {
                pump();
                std::thread::sleep(Duration::from_millis(400));
                pump();
                let out = std::process::Command::new("/usr/bin/import")
                    .args(["-window", "root"])
                    .arg(dir.join(name))
                    .output()
                    .expect("import");
                assert!(out.status.success(), "frame capture: {}", String::from_utf8_lossy(&out.stderr));
            }
        };
        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let pending = || state.borrow().guide_pending.clone();
        let put = |name: &str, label: &str| -> GuideRequest {
            let tmp = gdir.join("wayfinder.json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, label) };
            wait_for("guide request", || pending() == want);
            pump();
            want
        };
        let put_raw = |text: &str, want: GuideRequest| {
            std::fs::write(&file, text).unwrap();
            wait_for("guide request (raw)", || pending() == want);
            pump();
        };
        let tree = || all_widgets(ui.window.upcast_ref());
        let rings = || -> Vec<gtk::Widget> { guide_marked(tree()) };
        let label_pops = || -> Vec<gtk::Popover> {
            tree()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Popover>().ok())
                .filter(|p| p.has_css_class("wf-guide-label"))
                .collect()
        };
        let label_text = |p: &gtk::Popover| -> String {
            all_widgets(p.upcast_ref())
                .iter()
                .find_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.label().to_string()))
                .expect("label text")
        };
        let label_texts = || -> Vec<String> { label_pops().iter().map(|p| label_text(p)).collect() };
        let no_visual = |why: &str| {
            assert!(rings().is_empty(), "{why}: a ring is still drawn");
            assert!(label_pops().is_empty(), "{why}: a label is still drawn");
        };
        let by_name = |n: &str| -> Vec<gtk::Widget> { tree().into_iter().filter(|w| w.widget_name() == n).collect() };

        no_visual("no request");
        let xdo = |args: &[&str]| {
            let out = std::process::Command::new("/usr/bin/xdotool").args(args).output().expect("xdotool");
            assert!(out.status.success(), "xdotool {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let geo = xdo(&["search", "--name", "wf-d52-proof", "getwindowgeometry", "--shell"]);
        let num = |key: &str| -> i32 {
            geo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse().ok()).expect(key)
        };
        let (wx, wy) = (num("X="), num("Y="));
        // D-63: everything the mark must not change, read before, during and after.
        let term0 = by_name("terminal-here-folder").pop().expect("terminal button");
        let geometry = |w: &gtk::Widget| {
            (
                w.compute_bounds(&ui.window).map(|b| (b.x(), b.y(), b.width(), b.height())),
                (w.width(), w.height(), w.is_focusable(), w.can_target(), w.is_sensitive()),
                (ui.window.width(), ui.window.height()),
                GtkWindowExt::focus(&ui.window),
            )
        };
        let g_before = geometry(&term0);
        // Magenta stroke pixels in a crop of the captured root frame (guide magenta #ff2fd0).
        let magenta = |png: &std::path::Path, x: i32, y: i32, w: i32, h: i32| -> f64 {
            let out = std::process::Command::new("/usr/bin/magick")
                .arg(png)
                .args(["-crop", &format!("{w}x{h}+{x}+{y}"), "+repage", "-fuzz", "8%", "-fill", "black", "+opaque", "#ff2fd0"])
                .args(["-fill", "white", "-opaque", "#ff2fd0", "-format", "%[fx:mean*w*h]", "info:"])
                .output()
                .expect("magick");
            assert!(out.status.success(), "magick crop: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().parse().expect("pixel count")
        };


        // D-64: screen rectangle of any widget, in the main window or inside its own popover surface.
        let rect_of = |w: &gtk::Widget| -> (i32, i32, i32, i32) {
            use glib::translate::ToGlibPtr;
            if let Some(pop) = w.ancestor(gtk::Popover::static_type()).and_then(|p| p.downcast::<gtk::Popover>().ok()) {
                let native = pop.native().expect("popover native");
                let surface = native.surface().expect("popover surface");
                let xid = unsafe { gdk_x11_surface_get_xid(surface.to_glib_none().0) };
                let g = xdo(&["getwindowgeometry", "--shell", &xid.to_string()]);
                let n = |k: &str| -> i32 { g.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.trim().parse().ok()).expect(k) };
                let (tx, ty) = native.surface_transform();
                let b = w.compute_bounds(&pop).expect("bounds in popover");
                (n("X=") + (tx + b.x() as f64) as i32, n("Y=") + (ty + b.y() as f64) as i32, b.width() as i32, b.height() as i32)
            } else {
                let b = w.compute_bounds(&ui.window).expect("bounds");
                let (sx, sy) = gtk::prelude::NativeExt::surface_transform(&ui.window);
                (wx + (sx + b.x() as f64) as i32, wy + (sy + b.y() as f64) as i32, b.width() as i32, b.height() as i32)
            }
        };
        let arrow_pops = || -> Vec<gtk::Popover> {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-arrow")).collect()
        };
        let d64_png = crate::native_core::scratch_root().join(format!("wf-d64-{}.png", std::process::id()));
        // D-64: the one arrow must be a broad magenta head and shaft whose tip touches the row from the
        // side GTK actually placed it. Returns the pixel just inside the tip, where the arrow covers the row.
        let arrow_check = |key: &str, row: &gtk::Widget, frame_name: &str| -> (i32, i32) {
            for _ in 0..4 {
                pump();
            }
            let pops = arrow_pops();
            assert_eq!(pops.len(), 1, "{key}: exactly one arrow surface");
            assert!(!pops[0].can_target() && !pops[0].is_focusable() && !pops[0].can_focus(), "{key}: the arrow cannot take input");
            assert!(
                all_widgets(pops[0].upcast_ref())
                    .iter()
                    .filter(|w| w.is::<gtk::Label>() || w.is::<gtk::DrawingArea>() || w.is::<gtk::Overlay>())
                    .all(|w| !w.can_target() && !w.is_focusable()),
                "{key}: nothing inside the arrow can take input"
            );
            let area = all_widgets(pops[0].upcast_ref()).into_iter().find(|w| w.is::<gtk::DrawingArea>()).expect("arrow drawing");
            frame(frame_name);
            let (ax, ay, aw, ah) = rect_of(&area);
            let (rx, ry, rw, rh) = rect_of(row);
            let tip_left = rx + rw / 2 < ax + aw / 2;
            let (head, shaft) = (GUIDE_ARROW_HEAD as i32, GUIDE_ARROW_SHAFT as i32);
            assert_eq!(ah, GUIDE_ARROW_H as i32, "{key}: the head is the full arrow height");
            let tip_x = if tip_left { ax } else { ax + aw - 1 };
            let tip_y = ay + ah / 2;
            let edge = if tip_left { rx + rw } else { rx };
            let depth = if tip_left { edge - tip_x } else { tip_x - edge + 1 };
            assert!(
                (1..=GUIDE_ARROW_OVERLAP + 1).contains(&depth),
                "{key}: the tip lies over the row edge by {depth} px (arrow {ax},{ay} {aw}x{ah}, row {rx},{ry} {rw}x{rh}, tip_left {tip_left})"
            );
            assert!((tip_y - (ry + rh / 2)).abs() <= 3, "{key}: the tip is at the row's vertical middle: {tip_y} vs {}", ry + rh / 2);
            let cap = std::process::Command::new("/usr/bin/import").args(["-window", "root"]).arg(&d64_png).output().expect("import");
            assert!(cap.status.success(), "arrow capture: {}", String::from_utf8_lossy(&cap.stderr));
            let (hx, sx_) = if tip_left { (ax + head - 2, ax + aw - 2) } else { (ax + aw - 1 - (head - 2), ax + 1) };
            let head_px = magenta(&d64_png, hx, ay, 1, ah);
            let shaft_px = magenta(&d64_png, sx_, ay, 1, ah);
            let tip_px = magenta(&d64_png, if tip_left { tip_x } else { tip_x - 3 }, tip_y - 1, 4, 3);
            assert!(head_px >= (ah - 7) as f64, "{key}: broad triangular head: {head_px} magenta px in its base column");
            assert!(
                (shaft as f64 - 3.0..=shaft as f64 + 3.0).contains(&shaft_px),
                "{key}: broad shaft of {shaft} px: {shaft_px}"
            );
            assert!(tip_px >= 3.0, "{key}: the tip is drawn: {tip_px}");
            let covered = if tip_left { tip_x + 2 } else { tip_x - 2 };
            assert!(magenta(&d64_png, covered, tip_y, 1, 1) > 0.5, "{key}: the pixel just inside the tip is magenta");
            assert!(covered >= rx && covered < rx + rw, "{key}: that pixel lies over the row itself");
            (covered, tip_y)
        };
        let click_at = |x: i32, y: i32| {
            xdo(&["mousemove", &x.to_string(), &y.to_string()]);
            pump();
            xdo(&["mousedown", "1"]);
            pump();
            xdo(&["mouseup", "1"]);
            pump();
            pump();
        };

        frame("d52-0-before.png");

        // Static target: one ring, one visible label with the exact text, anchored to it.
        put("terminal-here-folder", "Open a terminal here");
        let term = by_name("terminal-here-folder").pop().expect("terminal button");
        assert_eq!(rings(), vec![term.clone()], "exactly the one real target is ringed");
        // D-63: the registry entry, not the size or type, chose the circle for this target.
        assert_eq!(guide_mark_for("terminal-here-folder"), Some(GuideMark::Circle));
        assert_eq!(guide_mark_for("trash-and-restore-file"), Some(GuideMark::Circle));
        assert_eq!(guide_mark_for("open-with-other-app"), Some(GuideMark::Arrow), "a row entry is an arrow");
        assert_eq!(guide_mark_for("no-such-control"), None, "no entry, no mark");
        assert!(term.has_css_class("wf-guide-circle"), "the compact target carries the circle class");
        assert!(label_pops().iter().all(|p| !p.has_css_class("wf-guide-arrow")), "a circle entry draws no arrow");
        assert_eq!(geometry(&term0), g_before, "drawing the circle moved nothing and took no focus");
        let pops = label_pops();
        assert_eq!(pops.len(), 1);
        assert_eq!(label_text(&pops[0]), "Open a terminal here");
        assert_eq!(pops[0].parent().as_ref(), Some(&term), "label anchored to the target");
        assert!(pops[0].is_visible());
        frame("d52-1-terminal-here-folder.png");
        {
            // The circle is on the button itself: stroke pixels sit inside its bounds on all four
            // sides, the icon centre stays clear, and the label is outside the button.
            let shot = crate::native_core::scratch_root().join(format!("wf-d63-crop-{}.png", std::process::id()));
            pump();
            std::thread::sleep(Duration::from_millis(400));
            pump();
            let cap = std::process::Command::new("/usr/bin/import").args(["-window", "root"]).arg(&shot).output().expect("import");
            assert!(cap.status.success(), "crop capture: {}", String::from_utf8_lossy(&cap.stderr));
            let b = term.compute_bounds(&ui.window).expect("terminal bounds");
            // The X window includes the client-side shadow; the surface transform is that margin.
            let (sx, sy) = gtk::prelude::NativeExt::surface_transform(&ui.window);
            let (bx, by, bw, bh) = (wx + (sx + b.x() as f64) as i32, wy + (sy + b.y() as f64) as i32, b.width() as i32, b.height() as i32);
            let whole = magenta(&shot, bx, by, bw, bh);
            let (top, bottom) = (magenta(&shot, bx, by, bw, bh / 6), magenta(&shot, bx, by + bh - bh / 6, bw, bh / 6));
            let (left, right) = (magenta(&shot, bx, by, bw / 6, bh), magenta(&shot, bx + bw - bw / 6, by, bw / 6, bh));
            let centre = magenta(&shot, bx + bw / 4, by + bh / 4, bw / 2, bh / 2);
            let _ = std::fs::remove_file(&shot);
            assert!(whole > 150.0, "stroke pixels inside the button bounds: {whole}");
            assert!(top > 0.0 && bottom > 0.0 && left > 0.0 && right > 0.0, "stroke on every side: {top} {bottom} {left} {right}");
            assert_eq!(centre, 0.0, "the icon centre is left clear: {centre} (crop {bx},{by} {bw}x{bh}, window {wx},{wy})");
        }

        // Replacement: the old visual is gone, the new one is the only one.
        let old_label = pops[0].clone();
        drop(pops);
        put("trash-and-restore-file", "Trash it");
        let trash = by_name("trash-and-restore-file").pop().expect("trash button");
        assert!(!term.has_css_class("wf-guide-circle"), "the old mark is gone");
        assert!(trash.has_css_class("wf-guide-circle"), "the second compact entry is a circle too");
        assert!(old_label.parent().is_none(), "the old label surface is gone");
        assert_eq!(rings(), vec![trash.clone()]);
        assert_eq!(label_texts(), vec!["Trash it".to_string()]);

        // An empty label is drawn exactly.
        put("trash-and-restore-file", "");
        assert_eq!(rings(), vec![trash.clone()]);
        assert_eq!(label_texts(), vec![String::new()]);

        // Every non-target state has no visual.
        put("", "");
        no_visual("clear");
        assert_eq!(geometry(&term0), g_before, "clearing the circle left the control exactly as before");
        put("terminal-here-folder", "again");
        assert_eq!(rings().len(), 1);
        put_raw("{not json", GuideRequest::Clear);
        no_visual("malformed");
        put("terminal-here-folder", "again");
        put_raw(
            r#"{"highlight":"no-such-control","label":"Hmm"}"#,
            GuideRequest::Clear,
        );
        no_visual("unknown");
        put("open-with-other-app", "no menu yet");
        no_visual("zero matches");
        put("terminal-here-folder", "again");
        assert_eq!(rings().len(), 1);
        trash.set_widget_name("terminal-here-folder");
        put("terminal-here-folder", "dup");
        no_visual("duplicate matches");
        trash.set_widget_name("trash-and-restore-file");
        put("terminal-here-folder", "back");
        assert_eq!(rings(), vec![term.clone()]);
        put("", "");
        no_visual("cleared again");

        // Real pointer through the visual: the ordinary click still fires once, nothing moves.
        let b = term.compute_bounds(&ui.window).expect("terminal bounds");
        let (cx, cy) = (b.x() + b.width() / 2.0, b.y() + b.height() / 2.0);
        let clicks = Rc::new(std::cell::Cell::new(0));
        {
            let clicks = clicks.clone();
            let term = term.clone().downcast::<gtk::Button>().expect("button");
            term.connect_clicked(move |_| clicks.set(clicks.get() + 1));
        }
        let old_path = std::env::var("PATH").ok();
        std::env::set_var("PATH", "");
        let click = || {
            xdo(&["mousemove", &(wx + cx as i32).to_string(), &(wy + cy as i32).to_string()]);
            pump();
            xdo(&["mousedown", "1"]);
            pump();
            xdo(&["mouseup", "1"]);
            pump();
        };
        let shape = || {
            let t = &term;
            (
                (ui.window.width(), ui.window.height()),
                (ui.files_stack.width(), ui.files_stack.height()),
                (t.width(), t.height(), t.is_focusable(), t.can_target(), t.is_sensitive()),
                t.compute_bounds(&ui.window).map(|b| (b.x(), b.y(), b.width(), b.height())),
            )
        };
        // Baseline with no request: what an ordinary click does to focus.
        put("", "");
        click();
        assert_eq!(clicks.get(), 1, "ordinary click fires once");
        let plain_focus = GtkWindowExt::focus(&ui.window);
        assert!(ui.status.label().contains("Terminal failed"), "the real handler ran: {}", ui.status.label());
        // Same click with the visual up.
        put("terminal-here-folder", "Click me");
        let (before, focus_before) = (shape(), GtkWindowExt::focus(&ui.window));
        assert_eq!(rings(), vec![term.clone()]);
        let pop = label_pops().pop().expect("label");
        assert!(!pop.can_target() && !pop.is_focusable() && !pop.can_focus(), "label cannot take input");
        let inner = all_widgets(pop.upcast_ref());
        assert!(inner.iter().filter(|w| w.is::<gtk::Label>()).all(|w| !w.can_target() && !w.is_focusable()));
        let hit = ui.window.pick(cx as f64, cy as f64, gtk::PickFlags::DEFAULT).expect("something under the pointer");
        assert!(hit == term || hit.is_ancestor(&term), "the pointer reaches the real control, not the visual");
        assert!(!inner.contains(&hit));
        assert_eq!(shape(), before, "showing the visual moved nothing");
        assert_eq!(GtkWindowExt::focus(&ui.window), focus_before, "showing the visual moved focus");
        click();
        assert_eq!(clicks.get(), 2, "the click through the visual fired exactly once");
        assert_eq!(GtkWindowExt::focus(&ui.window), plain_focus, "same focus result as an unguided click");
        assert_eq!(shape(), before, "the click moved nothing");
        assert_eq!(pending(), GuideRequest::Clear, "the click used the guided control (D-53)");
        no_visual("after the guided click");
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }

        // The real file menu: its live row alone is ringed, the menu stays open.
        let has_right_click = |w: &gtk::Widget| {
            let c = w.observe_controllers();
            (0..c.n_items()).any(|i| {
                c.item(i).and_then(|o| o.downcast::<gtk::GestureClick>().ok()).is_some_and(|g| g.button() == 3)
            })
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        let open_menu = || {
            show_item_context_menu(&row, state.clone(), ui.clone(), entry.clone(), 5.0, 5.0);
            pump();
        };
        let close_menu = || {
            for pop in popovers_of(&row) {
                pop.popdown();
                pop.unparent();
            }
            pump();
        };
        put("open-with-other-app", "Open With here");
        no_visual("pending before the menu");
        open_menu();
        let first = by_name("open-with-other-app").pop().expect("opener");
        assert_eq!(rings(), vec![first.clone()], "the live row alone is ringed");
        let pops = label_pops();
        assert_eq!(pops.len(), 1);
        assert_eq!(label_text(&pops[0]), "Open With here");
        assert_eq!(pops[0].parent().as_ref(), Some(&first));
        let item_menu = popovers_of(&row).into_iter().find(|p| !p.has_css_class("wf-guide-label")).expect("item menu");
        assert!(item_menu.is_mapped(), "the parent menu stays open under the visual");
        assert!(pops[0].has_css_class("wf-guide-arrow"), "the row entry draws the arrow surface");
        assert!(!first.has_css_class("wf-guide-circle"), "and no circle class on the row");
        arrow_check("open-with-other-app", &first, "d52-2-open-with-other-app.png");
        let stale_label = pops[0].clone();
        drop((pops, item_menu));
        close_menu();
        no_visual("closed menu");
        assert!(stale_label.parent().is_none());
        assert_eq!(pending(), known("open-with-other-app", "Open With here"), "still pending");
        drop(first);
        open_menu();
        let second = by_name("open-with-other-app").pop().expect("new opener");
        assert_eq!(rings(), vec![second.clone()], "the fresh row is ringed");
        assert_eq!(label_texts(), vec!["Open With here".to_string()]);
        drop(second);
        close_menu();
        no_visual("closed again");

        // D-73: the item menu's one real scroller takes the arrow; the menu itself never changes.
        put("item-menu-cap-d56", "Whole menu");
        no_visual("pending before the item menu exists");
        assert_eq!(pending(), known("item-menu-cap-d56", "Whole menu"));
        open_menu();
        let scrollers = by_name("item-menu-cap-d56");
        assert_eq!(scrollers.len(), 1, "exactly one real scroller is named");
        let scroller = scrollers[0].clone();
        assert!(scroller.is::<gtk::ScrolledWindow>() && scroller.is_mapped());
        let item_menu = popovers_of(&row).into_iter().find(|p| !p.has_css_class("wf-guide-label")).expect("item menu");
        assert!(scroller.is_ancestor(&item_menu), "it is the menu popover's own scroller");
        assert_eq!(item_menu.child().as_ref(), Some(&scroller), "and the popover's child");
        assert!(button_labelled(item_menu.upcast_ref(), "Open Terminal Here").is_some(), "Open Terminal Here exists");
        let sw = scroller.downcast_ref::<gtk::ScrolledWindow>().unwrap().clone();
        let menu_shape = || (item_menu.width(), item_menu.height(), sw.width(), sw.height(), sw.vadjustment().upper(), sw.vadjustment().page_size(), sw.vadjustment().value());
        let shape_open = menu_shape();
        let pops = label_pops();
        assert_eq!(pops.len(), 1);
        assert_eq!(label_text(&pops[0]), "Whole menu");
        assert_eq!(pops[0].parent().as_ref(), Some(&scroller));
        assert!(pops[0].has_css_class("wf-guide-arrow"));
        wait_for("the arrow surface", || {
            label_pops().first().is_some_and(|p| p.is_mapped() && p.native().and_then(|n| n.surface()).is_some())
        });
        arrow_check("item-menu-cap-d56", &scroller, "d73-item-menu-cap-d56.png");
        assert_eq!(menu_shape(), shape_open, "the arrow moved or resized nothing in the menu");
        assert!(item_menu.is_mapped(), "the menu stays open under the arrow");
        assert!(button_labelled(item_menu.upcast_ref(), "Open Terminal Here").is_some_and(|b| b.is_mapped() && b.can_target()));
        assert_eq!(by_name("item-menu-cap-d56").len(), 1, "no duplicate target");
        let stale = pops[0].clone();
        drop((pops, item_menu, sw, scrollers));
        close_menu();
        no_visual("closed item menu");
        assert!(stale.parent().is_none() && guide_live_target(&state).is_none(), "no stale visual or target");
        assert_eq!(pending(), known("item-menu-cap-d56", "Whole menu"), "still pending after close");
        drop(scroller);
        open_menu();
        let again = by_name("item-menu-cap-d56");
        assert_eq!(again.len(), 1, "reopening gives one new scroller");
        assert_eq!(label_pops().len(), 1, "one arrow visual on the new scroller");
        drop(again);
        put("", "");
        no_visual("cleared");
        close_menu();

        // D-74: the item menu's one real content box takes the arrow; the menu itself never changes.
        put("item-menu-full-d60", "Every action");
        no_visual("pending before the item menu exists (content)");
        assert_eq!(pending(), known("item-menu-full-d60", "Every action"));
        open_menu();
        let contents = by_name("item-menu-full-d60");
        assert_eq!(contents.len(), 1, "exactly one real content box is named");
        let content = contents[0].clone();
        assert!(content.is::<gtk::Box>() && content.is_mapped());
        let item_menu = popovers_of(&row).into_iter().find(|p| !p.has_css_class("wf-guide-label")).expect("item menu");
        assert!(content.is_ancestor(&item_menu), "it is inside the menu popover");
        let sw = item_menu.child().and_then(|c| c.downcast::<gtk::ScrolledWindow>().ok()).expect("the D-73 scroller");
        assert!(content.is_ancestor(&sw), "and inside the real scroller");
        assert_eq!(sw.widget_name(), "item-menu-cap-d56", "the scroller keeps its own name");
        let menu_shape = || (item_menu.width(), item_menu.height(), sw.width(), sw.height(), sw.vadjustment().upper(), sw.vadjustment().page_size(), sw.vadjustment().value());
        let shape_open = menu_shape();
        let pops = label_pops();
        assert_eq!(pops.len(), 1);
        assert_eq!(label_text(&pops[0]), "Every action");
        assert_eq!(pops[0].parent().as_ref(), Some(&content));
        assert!(pops[0].has_css_class("wf-guide-arrow"));
        wait_for("the content arrow surface", || {
            label_pops().first().is_some_and(|p| p.is_mapped() && p.native().and_then(|n| n.surface()).is_some())
        });
        arrow_check("item-menu-full-d60", &content, "d74-item-menu-full-d60.png");
        assert_eq!(menu_shape(), shape_open, "the arrow moved or resized nothing in the menu");
        let adj = sw.vadjustment();
        assert!(adj.upper() <= adj.page_size() + 0.5 && adj.value() == 0.0, "scroll range equals the visible page");
        assert!(!sw.vscrollbar().is_mapped(), "no vertical scrollbar");
        let top = content.first_child().expect("first action");
        assert!(top.is_mapped() && top.can_target(), "first action mapped and targetable");
        assert!(button_labelled(item_menu.upcast_ref(), "Open Terminal Here").is_some_and(|b| b.is_mapped() && b.can_target()));
        let bottom = button_labelled(item_menu.upcast_ref(), "Open Terminal Here").unwrap();
        let bounds = bottom.compute_bounds(&content).expect("bounds");
        assert!(bounds.y() >= 0.0 && bounds.y() + bounds.height() <= content.height() as f32 + 0.5, "top and bottom visible together");
        assert!(item_menu.is_mapped(), "the menu stays open under the arrow");
        assert_eq!(by_name("item-menu-full-d60").len(), 1, "no duplicate target");
        let stale = pops[0].clone();
        drop((pops, item_menu, sw, adj, top, bottom, contents));
        close_menu();
        no_visual("closed item menu (content)");
        assert!(stale.parent().is_none() && guide_live_target(&state).is_none(), "no stale visual or target");
        assert_eq!(pending(), known("item-menu-full-d60", "Every action"), "still pending after close");
        drop(content);
        open_menu();
        let again = by_name("item-menu-full-d60");
        assert_eq!(again.len(), 1, "reopening gives one new content box");
        assert_eq!(label_pops().len(), 1, "one arrow visual on the new content box");
        drop(again);
        put("", "");
        no_visual("cleared (content)");
        close_menu();

        // D-64: circles and arrows replace one another cleanly, and the row never changes.
        put("", "");
        open_menu();
        let opener = by_name("open-with-other-app").pop().expect("opener");
        let g0 = geometry(&opener);
        put("open-with-other-app", "Click the tip");
        assert_eq!(rings(), vec![opener.clone()]);
        assert_eq!(arrow_pops().len(), 1);
        assert_eq!(geometry(&opener), g0, "during: the arrow moved, resized or refocused nothing");
        put("terminal-here-folder", "circle now");
        assert!(arrow_pops().is_empty(), "the arrow is gone when a circle entry replaces it");
        assert_eq!(rings(), vec![term.clone()], "one circle and no arrow");
        put("open-with-other-app", "Click the tip");
        assert_eq!(arrow_pops().len(), 1);
        assert!(!term.has_css_class("wf-guide-circle"), "the arrow entry replaced the circle");
        put_raw("{not json", GuideRequest::Clear);
        no_visual("malformed after an arrow");
        put("open-with-other-app", "Click the tip");
        put_raw(r#"{"highlight":"no-such-control","label":"Hmm"}"#, GuideRequest::Clear);
        no_visual("unknown after an arrow");
        put("open-with-other-app", "Click the tip");
        put("", "");
        no_visual("clear after an arrow");
        assert_eq!(geometry(&opener), g0, "after clearing: nothing was left changed");
        // A real pointer press at a pixel the arrow tip covers reaches the row exactly once.
        put("open-with-other-app", "Click the tip");
        let (cx_, cy_) = arrow_check("tip click", &opener, "d52-3-tip-click.png");
        let taps = Rc::new(std::cell::Cell::new(0));
        {
            let taps = taps.clone();
            opener.downcast_ref::<gtk::Button>().expect("a real button").connect_clicked(move |_| taps.set(taps.get() + 1));
        }
        click_at(cx_, cy_);
        assert_eq!(taps.get(), 1, "the click at the arrow tip fired the row action exactly once");
        assert_eq!(pending(), GuideRequest::Clear, "and the guide cleared");
        no_visual("after the tip click");
        drop(opener);
        close_menu();
        no_visual("menu closed after the tip click");

        // Drive menus: a mounted and an unmounted real target show without closing their menu.
        let find_sidebar_row = || {
            all_widgets(ui.sidebar.upcast_ref())
                .into_iter()
                .find_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
                .expect("a sidebar row")
        };
        let drive = |name: &str, removable: bool, mounted: bool| Drive {
            name: name.to_string(),
            path: format!("/dev/synthetic-{name}"),
            mountpoint: if mounted { "/nonexistent/synthetic".into() } else { String::new() },
            size: 1,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable,
            space: None,
        };
        for (key, d) in [
            ("drive-open-crucial", drive("Launch Box SSD 2TB", false, true)),
            ("drive-remount-stick", drive("Stick", true, false)),
        ] {
            put(key, "Drive step");
            no_visual("drive pending");
            let sidebar_row = find_sidebar_row();
            show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), d, 5.0, 5.0);
            pump();
            let target = by_name(key).pop().expect("drive target");
            assert_eq!(rings(), vec![target.clone()], "{key} is ringed");
            assert_eq!(label_texts(), vec!["Drive step".to_string()]);
            let menu = popovers_of(sidebar_row.upcast_ref())
                .into_iter()
                .find(|p| !p.has_css_class("wf-guide-label"))
                .expect("drive menu");
            assert!(menu.is_mapped(), "{key}: the menu stays open");
            let (cx_, cy_) = arrow_check(key, &target, &format!("d52-4-{key}.png"));
            if key == "drive-open-crucial" {
                let taps = Rc::new(std::cell::Cell::new(0));
                {
                    let taps = taps.clone();
                    target.downcast_ref::<gtk::Button>().expect("a real button").connect_clicked(move |_| taps.set(taps.get() + 1));
                }
                click_at(cx_, cy_);
                assert_eq!(taps.get(), 1, "{key}: the click at the arrow tip fired the row action once");
                assert_eq!(pending(), GuideRequest::Clear, "{key}: and the guide cleared");
                no_visual("drive tip click");
            }
            drop((target, menu));
            for pop in popovers_of(sidebar_row.upcast_ref()) {
                pop.popdown();
                pop.unparent();
            }
            pump();
            no_visual("drive menu closed");
        }

        // Teardown: closing the window with an arrow up removes it.
        put("drive-remount-stick", "teardown");
        let sidebar_row = find_sidebar_row();
        show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), drive("Stick", true, false), 5.0, 5.0);
        pump();
        assert_eq!(arrow_pops().len(), 1);
        let weak = arrow_pops()[0].downgrade();
        ui.window.destroy();
        pump();
        assert!(weak.upgrade().is_none_or(|p| p.parent().is_none()), "window teardown removed the arrow");
        let _ = std::fs::remove_file(&d64_png);
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        TEST_CHILD_FAILS.with(|f| f.set(false));
    }

    #[cfg(feature = "guide")]
    extern "C" {
        fn gdk_x11_surface_get_xid(surface: *mut gdk::ffi::GdkSurface) -> std::os::raw::c_ulong;
    }

    #[cfg(feature = "guide")]
    fn guide_clears_on_use_d53() {
        use glib::translate::ToGlibPtr;
        use std::os::unix::fs::MetadataExt;
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        // D-57: the launch seam succeeds by default; these proofs need the failing handler.
        TEST_CHILD_FAILS.with(|f| f.set(true));
        let old_path = std::env::var("PATH").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d53-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let gdir = root.join("guide");
        for d in [&home, &dir, &gdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        TEST_OPEN_WITH_APPS.with(|a| {
            *a.borrow_mut() = Some(vec![native_core::AppEntry { id: "fake-editor-a.desktop".into(), name: "Fake Editor A".into() }])
        });
        TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().clear());
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d53test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.set_title(Some("wf-d53-proof"));
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let frames = std::env::var("WF_D53_FRAMES").ok().map(std::path::PathBuf::from);
        let frame = |name: &str| {
            if let Some(dir) = &frames {
                pump();
                std::thread::sleep(Duration::from_millis(400));
                pump();
                let out = std::process::Command::new("/usr/bin/import")
                    .args(["-window", "root"])
                    .arg(dir.join(name))
                    .output()
                    .expect("import");
                assert!(out.status.success(), "frame capture: {}", String::from_utf8_lossy(&out.stderr));
            }
        };
        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let pending = |s: &Rc<RefCell<AppState>>| s.borrow().guide_pending.clone();
        let put = |name: &str, label: &str| {
            let tmp = gdir.join("wayfinder.json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, label) };
            wait_for("guide request", || pending(&state) == want);
            pump();
        };
        let tree = || all_widgets(ui.window.upcast_ref());
        let rings = || -> Vec<gtk::Widget> { guide_marked(tree()) };
        let labels = || -> usize {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-label")).count()
        };
        let by_name = |n: &str| -> Vec<gtk::Widget> { tree().into_iter().filter(|w| w.widget_name() == n).collect() };
        let is_painted = |s: &Rc<RefCell<AppState>>| s.borrow().guide_visual.is_some();
        let cleared = |why: &str| {
            assert_eq!(pending(&state), GuideRequest::Clear, "{why}: request cleared");
            assert!(state.borrow().guide_target.is_none(), "{why}: target dropped");
            assert!(!is_painted(&state), "{why}: visual dropped");
            assert!(rings().is_empty(), "{why}: ring removed");
            assert_eq!(labels(), 0, "{why}: label removed");
        };
        let file_sig = || {
            let m = std::fs::metadata(&file).unwrap();
            (std::fs::read(&file).unwrap(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.mode())
        };

        let xdo = |args: &[&str]| {
            let out = std::process::Command::new("/usr/bin/xdotool").args(args).output().expect("xdotool");
            assert!(out.status.success(), "xdotool {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let geometry = |target: &str| -> (i32, i32) {
            let geo = xdo(&["getwindowgeometry", "--shell", target]);
            let num = |key: &str| -> i32 {
                geo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse().ok()).expect(key)
            };
            (num("X="), num("Y="))
        };
        let win_id = xdo(&["search", "--name", "wf-d53-proof"]).lines().next().expect("window id").to_string();
        let (wx, wy) = geometry(&win_id);
        // Absolute centre of a control, in the main window or inside its own popover surface.
        let centre = |w: &gtk::Widget| -> (i32, i32) {
            if let Some(pop) = w.ancestor(gtk::Popover::static_type()).and_then(|p| p.downcast::<gtk::Popover>().ok()) {
                let native = pop.native().expect("popover native");
                let surface = native.surface().expect("popover surface");
                let xid = unsafe { gdk_x11_surface_get_xid(surface.to_glib_none().0) };
                let (gx, gy) = geometry(&xid.to_string());
                let (tx, ty) = native.surface_transform();
                let b = w.compute_bounds(&pop).expect("bounds in popover");
                (gx + (tx + (b.x() + b.width() / 2.0) as f64) as i32, gy + (ty + (b.y() + b.height() / 2.0) as f64) as i32)
            } else {
                let b = w.compute_bounds(&ui.window).expect("bounds");
                (wx + (b.x() + b.width() / 2.0) as i32, wy + (b.y() + b.height() / 2.0) as i32)
            }
        };
        let click = |w: &gtk::Widget| {
            let (x, y) = centre(w);
            xdo(&["mousemove", &x.to_string(), &y.to_string()]);
            pump();
            xdo(&["mousedown", "1"]);
            pump();
            xdo(&["mouseup", "1"]);
            pump();
        };
        // The keyboard route: focus the control with the pointer over it, press Enter.
        let key = |w: &gtk::Widget| {
            let (x, y) = centre(w);
            xdo(&["mousemove", &x.to_string(), &y.to_string()]);
            pump();
            w.grab_focus();
            pump();
            xdo(&["key", "Return"]);
            pump();
        };
        let observe = |w: &gtk::Widget| -> Rc<std::cell::Cell<u32>> {
            let n = Rc::new(std::cell::Cell::new(0));
            let counter = n.clone();
            w.downcast_ref::<gtk::Button>().expect("a real button").connect_clicked(move |_| counter.set(counter.get() + 1));
            n
        };
        std::env::set_var("PATH", "");

        // Static toolbar target, real pointer, then real keyboard.
        for route in ["pointer", "keyboard"] {
            put("terminal-here-folder", "Open a terminal here");
            let term = by_name("terminal-here-folder").pop().expect("terminal button");
            assert_eq!(rings(), vec![term.clone()]);
            assert_eq!(labels(), 1);
            let seen = observe(&term);
            ui.status.set_label("");
            let before = file_sig();
            if route == "pointer" {
                click(&term);
            } else {
                key(&term);
            }
            assert_eq!(seen.get(), 1, "{route}: the ordinary click fired once");
            assert!(ui.status.label().contains("Terminal failed"), "{route}: the real handler ran: {}", ui.status.label());
            cleared(route);
            assert_eq!(file_sig(), before, "{route}: the guide file was not touched");
            // Same unchanged request rewritten: one fresh visual, and it clears once more.
            put("terminal-here-folder", "Open a terminal here");
            assert_eq!(rings(), vec![term.clone()], "{route}: fresh visual");
            assert_eq!(labels(), 1);
            put("", "");
            cleared("rewritten then cleared by the file");
        }

        // A stale target, an unguided button, a duplicate name: none of them clears the current request.
        put("terminal-here-folder", "one");
        let term = by_name("terminal-here-folder").pop().unwrap();
        let term_seen = observe(&term);
        put("trash-and-restore-file", "two");
        let stale_before = (pending(&state), rings().len(), labels());
        click(&term);
        assert_eq!(term_seen.get(), 1, "the old control still works once");
        assert_eq!((pending(&state), rings().len(), labels()), stale_before, "a stale target cannot clear");
        assert_eq!(pending(&state), known("trash-and-restore-file", "two"));
        let undo = tree()
            .into_iter()
            .find(|w| w.is::<gtk::Button>() && w.tooltip_text().as_deref() == Some("Undo"))
            .expect("undo button");
        let undo_seen = observe(&undo);
        undo.downcast_ref::<gtk::Button>().unwrap().emit_clicked();
        pump();
        assert_eq!(undo_seen.get(), 1);
        assert_eq!(pending(&state), known("trash-and-restore-file", "two"), "an unguided button cannot clear");
        assert_eq!(rings().len(), 1);
        let trash = by_name("trash-and-restore-file").pop().unwrap();
        trash.set_widget_name("terminal-here-folder");
        put("terminal-here-folder", "dup");
        assert!(rings().is_empty() && !is_painted(&state), "duplicate fails closed");
        click(&term);
        assert_eq!(pending(&state), known("terminal-here-folder", "dup"), "a duplicate-name failure cannot clear");
        trash.set_widget_name("trash-and-restore-file");
        put("", "");

        // A second window with the same matching control cannot clear the first window's request.
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state2, ui2) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("second window");
        ui2.window.set_title(Some("wf-d53-second"));
        ui2.window.present();
        wait_for("second listing", || state2.borrow().visible_entries.len() == 1);
        put("terminal-here-folder", "both windows");
        wait_for("second window request", || pending(&state2) == known("terminal-here-folder", "both windows"));
        let term2 = all_widgets(ui2.window.upcast_ref()).into_iter().find(|w| w.widget_name() == "terminal-here-folder").unwrap();
        assert_ne!(term2, by_name("terminal-here-folder").pop().unwrap());
        term2.downcast_ref::<gtk::Button>().unwrap().emit_clicked();
        pump();
        assert_eq!(pending(&state2), GuideRequest::Clear, "the second window's own use clears its own request");
        assert_eq!(pending(&state), known("terminal-here-folder", "both windows"), "the first window is unaffected");
        assert!(is_painted(&state) && rings().len() == 1);
        ui2.window.destroy();
        pump();
        put("", "");

        // Dynamic: the real file context menu, Open With, pointer then keyboard.
        let has_right_click = |w: &gtk::Widget| {
            let c = w.observe_controllers();
            (0..c.n_items()).any(|i| c.item(i).and_then(|o| o.downcast::<gtk::GestureClick>().ok()).is_some_and(|g| g.button() == 3))
        };
        // D-59: a row is only ever used in the same step it was found; put() pumps and can rebuild rows.
        let rooted = |w: &gtk::Widget| w.root().is_some_and(|r| r.upcast::<gtk::Widget>() == ui.window.clone().upcast::<gtk::Widget>());
        let file_row = || {
            all_widgets(ui.list.upcast_ref())
                .into_iter()
                .find(|w| w.is::<gtk::Box>() && has_right_click(w) && rooted(w))
                .expect("a rooted file row")
        };
        for route in ["pointer", "keyboard"] {
            let mut attempt = 0;
            let (row, opener, item_menu) = loop {
                attempt += 1;
                put("open-with-other-app", "Open With here");
                let row = file_row();
                ui.list_selection.select_item(0, true);
                update_selection(&state, &ui);
                let entry = state.borrow().visible_entries[0].clone();
                assert!(rooted(&row), "{route}: the file anchor is rooted in the window before its menu opens");
                show_item_context_menu(&row, state.clone(), ui.clone(), entry, 5.0, 5.0);
                pump();
                pump();
                let found = by_name("open-with-other-app").pop().filter(|t| t.is_mapped() && t.is_ancestor(&row));
                let menu = popovers_of(&row).into_iter().find(|p| !p.has_css_class("wf-guide-label"));
                if let (Some(opener), Some(item_menu)) = (found, menu) {
                    break (row, opener, item_menu);
                }
                assert!(attempt < 10, "{route}: the file menu never stood");
                close_menu_of(&row);
            };
            assert_eq!(rings(), vec![opener.clone()], "{route}: ringed before use");
            assert_eq!(labels(), 1);
            let seen = observe(&opener);
            let before = file_sig();
            if route == "pointer" {
                frame("d53-1-open-with-before.png");
                click(&opener);
                frame("d53-2-open-with-after.png");
            } else {
                key(&opener);
            }
            assert_eq!(seen.get(), 1, "{route}: Open With clicked once");
            cleared(route);
            assert_eq!(file_sig(), before);
            let sides: Vec<gtk::Popover> =
                popovers_of(opener.upcast_ref()).into_iter().filter(|p| !p.has_css_class("wf-guide-label")).collect();
            assert_eq!(sides.len(), 1, "{route}: the side menu opened exactly once");
            assert!(sides[0].is_mapped() && item_menu.is_mapped(), "{route}: both menus stay usable");
            assert!(button_labelled(sides[0].upcast_ref(), "Fake Editor A").is_some());
            assert!(TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow().is_empty()), "nothing launched");
            drop((sides, item_menu, opener));
            close_menu_of(&row);
            assert!(popovers_of(&row).is_empty(), "{route}: every popover of the attempt's anchor is closed");
        }

        // Dynamic: all six real drive buttons, synthetic drives, empty PATH so nothing real can run.
        let sidebar_row = || {
            all_widgets(ui.sidebar.upcast_ref())
                .into_iter()
                .filter(|w| rooted(w))
                .find_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
                .expect("a rooted sidebar row")
        };
        let drive = |name: &str, removable: bool, mounted: bool| Drive {
            name: name.to_string(),
            path: format!("/dev/synthetic-{name}"),
            mountpoint: if mounted { dir.to_string_lossy().to_string() } else { String::new() },
            size: 1,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable,
            space: None,
        };
        let crucial = |mounted| drive("Launch Box SSD 2TB", false, mounted);
        let stick = |mounted| drive("Stick", true, mounted);
        let cases = [
            ("drive-open-crucial", crucial(true)),
            ("drive-open-stick", stick(true)),
            ("drive-remount-crucial", crucial(false)),
            ("drive-remount-stick", stick(false)),
            ("drive-unmount-crucial", crucial(true)),
            ("drive-unmount-stick", stick(true)),
        ];
        for (key_name, d) in cases {
            for route in ["pointer", "keyboard"] {
                // A finished job refreshes the sidebar and can rebuild its rows under an open menu:
                // reopen on the live row until the menu really stands before it is used.
                let mut attempt = 0;
                let (sidebar_row, target, seen, before) = loop {
                    attempt += 1;
                    pump();
                    put(key_name, "Drive step");
                    let sidebar_row = sidebar_row();
                    assert!(rooted(sidebar_row.upcast_ref()), "{key_name}: the sidebar anchor is rooted before its menu opens");
                    ui.status.set_label("");
                    show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), d.clone(), 5.0, 5.0);
                    pump();
                    pump();
                    let found = by_name(key_name).pop();
                    if let Some(target) = found.filter(|t| t.is_mapped() && t.is_ancestor(&sidebar_row)) {
                        assert_eq!(rings(), vec![target.clone()], "{key_name}: ringed");
                        let seen = observe(&target);
                        break (sidebar_row, target, seen, file_sig());
                    }
                    assert!(attempt < 10, "{key_name}: the drive menu never stood");
                    close_menu_of(&sidebar_row);
                };
                if route == "pointer" {
                    click(&target);
                } else {
                    key(&target);
                }
                for _ in 0..5 {
                    pump();
                }
                assert_eq!(seen.get(), 1, "{key_name} {route}: clicked once");
                cleared(&format!("{key_name} {route}"));
                assert_eq!(file_sig(), before);
                let status = ui.status.label().to_string();
                if key_name.starts_with("drive-open") {
                    assert_eq!(state.borrow().cwd, dir.to_string_lossy(), "{key_name}: opened the Scratch folder");
                } else {
                    assert!(!status.is_empty(), "{key_name} {route}: the ordinary action ran (it failed safely with no PATH)");
                }
                close_menu_of(&sidebar_row);
                assert!(popovers_of(sidebar_row.upcast_ref()).is_empty(), "{key_name} {route}: the attempt's popovers are closed");
            }
        }

        // A later watcher event still repaints normally.
        put("terminal-here-folder", "after all");
        assert_eq!(rings().len(), 1);
        assert_eq!(labels(), 1);

        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        ui.window.destroy();
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        TEST_OPEN_WITH_APPS.with(|a| *a.borrow_mut() = None);
        TEST_CHILD_FAILS.with(|f| f.set(false));
    }

    #[cfg(feature = "guide")]
    fn close_menu_of(widget: &impl IsA<gtk::Widget>) {
        // D-59: keyboard focus left inside a menu would be moved by GTK's next paint after the unparent.
        if let Some(window) = widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
            gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        }
        for pop in popovers_of(widget.upcast_ref()) {
            pop.popdown();
            pop.unparent();
        }
        pump();
    }

    #[cfg(feature = "guide")]
    fn guide_pending_targets_resolve_d51() {
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d51-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let gdir = root.join("guide");
        for d in [&home, &dir, &gdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d51test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let pending = || state.borrow().guide_pending.clone();
        let target = || guide_live_target(&state);
        let mut serial = 0;
        let mut put = |name: &str| -> GuideRequest {
            serial += 1;
            let label = format!("L{serial}");
            let tmp = gdir.join("wayfinder.json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, &label) };
            wait_for("guide request", || pending() == want);
            want
        };
        let put_raw = |text: &str, want: GuideRequest| {
            std::fs::write(&file, text).unwrap();
            wait_for("guide request (raw)", || pending() == want);
        };
        // The D-52 ring/label surface is measured by the D-52 test, not here.
        let in_guide_label = |w: &gtk::Widget| {
            let mut cur = Some(w.clone());
            while let Some(c) = cur {
                if c.has_css_class("wf-guide-label") {
                    return true;
                }
                cur = c.parent();
            }
            false
        };
        let snapshot = || -> Vec<(i32, i32, bool, bool, bool)> {
            all_widgets(ui.window.upcast_ref())
                .iter()
                .filter(|w| !in_guide_label(w))
                .map(|w| (w.width(), w.height(), w.is_focusable(), w.can_target(), w.is_sensitive()))
                .collect()
        };
        let labels = || -> Vec<String> {
            all_widgets(ui.window.upcast_ref())
                .iter()
                .filter(|w| !in_guide_label(w))
                .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.label().to_string()))
                .collect()
        };
        let by_name = |n: &str| -> Vec<gtk::Widget> {
            all_widgets(ui.window.upcast_ref()).into_iter().filter(|w| w.widget_name() == n).collect()
        };

        // Lookup changes nothing: focus, allocations, focusability, targetability,
        // labels. Measured around the lookup call alone, with no main-loop turn between.
        assert!(target().is_none(), "no request, no target");
        pump();
        let focus_before = GtkWindowExt::focus(&ui.window);
        let (shape_before, labels_before) = (snapshot(), labels());
        state.borrow_mut().guide_pending = known("terminal-here-folder", "direct");
        guide_resolve(&state);
        assert_eq!(GtkWindowExt::focus(&ui.window), focus_before, "lookup moved focus");
        assert!(snapshot() == shape_before, "lookup changed a widget");
        assert!(labels() == labels_before, "lookup changed a label");
        put("terminal-here-folder");
        let toolbar = by_name("terminal-here-folder");
        assert_eq!(toolbar.len(), 1);
        assert_eq!(target().as_ref(), Some(&toolbar[0]), "a static request binds to its one real widget");
        drop(toolbar);

        // Clear, malformed and unknown requests each drop an old target.
        put("");
        assert!(target().is_none(), "clear leaves no target");
        put("terminal-here-folder");
        assert!(target().is_some());
        put_raw("{not json", GuideRequest::Clear);
        assert!(target().is_none(), "malformed leaves no target");
        put("terminal-here-folder");
        assert!(target().is_some());
        put_raw(
            r#"{"highlight":"no-such-control","label":"Hmm"}"#,
            GuideRequest::Clear,
        );
        assert!(target().is_none(), "unknown leaves no target");

        // A duplicated name fails closed and stays pending; removing the twin binds again.
        put("terminal-here-folder");
        assert!(target().is_some());
        let twin = by_name("trash-and-restore-file").pop().expect("the trash button");
        twin.set_widget_name("terminal-here-folder");
        let want = put("terminal-here-folder");
        assert!(target().is_none(), "two live widgets share the name: no choice by traversal order");
        assert_eq!(pending(), want, "the duplicate request stays pending");
        twin.set_widget_name("trash-and-restore-file");
        put("terminal-here-folder");
        assert!(target().is_some());

        // Atomic replacement to a dynamic name drops the old target and stays pending.
        let want = put("open-with-other-app");
        assert!(target().is_none(), "the old target went with the old request");
        assert_eq!(pending(), want, "zero matches stay pending");

        // The real file menu builds the opener; closing it lets the target go.
        let has_right_click = |w: &gtk::Widget| {
            let c = w.observe_controllers();
            (0..c.n_items()).any(|i| {
                c.item(i).and_then(|o| o.downcast::<gtk::GestureClick>().ok()).is_some_and(|g| g.button() == 3)
            })
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        let open_menu = || {
            show_item_context_menu(&row, state.clone(), ui.clone(), entry.clone(), 5.0, 5.0);
            pump();
        };
        let close_menu = || {
            for pop in popovers_of(&row) {
                pop.popdown();
                pop.unparent();
            }
            pump();
        };
        open_menu();
        let first = target().expect("the opener binds when its menu is built");
        assert_eq!(first.widget_name(), "open-with-other-app");
        assert!(first.is::<gtk::Button>() && first.is_ancestor(&ui.window));
        drop(first);
        close_menu();
        assert!(target().is_none(), "an unparented menu's target is unavailable");
        assert_eq!(pending(), want, "the request is kept for the next opening");
        open_menu();
        let second = target().expect("reopening binds the new real opener");
        assert_eq!(second.widget_name(), "open-with-other-app");
        drop(second);
        close_menu();

        // Drive menus: each of the six keys binds only in the menu state that has it.
        let sidebar_row = all_widgets(ui.sidebar.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
            .expect("a sidebar row");
        let drive = |name: &str, removable: bool, mounted: bool| Drive {
            name: name.to_string(),
            path: format!("synthetic:{name}"),
            mountpoint: if mounted { "/nonexistent/synthetic".into() } else { String::new() },
            size: 1,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable,
            space: None,
        };
        let crucial = "Launch Box SSD 2TB";
        let states: Vec<(Drive, Vec<&str>)> = vec![
            (drive(crucial, false, true), vec!["drive-open-crucial", "drive-unmount-crucial"]),
            (drive(crucial, false, false), vec!["drive-remount-crucial"]),
            (drive("Stick", true, true), vec!["drive-eject-stick", "drive-open-stick", "drive-unmount-stick"]),
            (drive("Stick", true, false), vec!["drive-eject-stick", "drive-remount-stick"]),
        ];
        let seven = [
            "drive-open-crucial", "drive-unmount-crucial", "drive-remount-crucial",
            "drive-open-stick", "drive-unmount-stick", "drive-remount-stick", "drive-eject-stick",
        ];
        for key in seven {
            let want = put(key);
            assert!(target().is_none(), "{key} is pending before any drive menu exists");
            for (d, has) in &states {
                show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), d.clone(), 5.0, 5.0);
                pump();
                let bound = target();
                assert_eq!(
                    bound.is_some(),
                    has.contains(&key),
                    "{key} in the {} mounted={} menu",
                    d.name,
                    !d.mountpoint.is_empty()
                );
                if let Some(b) = &bound {
                    assert_eq!(b.widget_name(), key);
                }
                drop(bound);
                for pop in popovers_of(sidebar_row.upcast_ref()) {
                    pop.popdown();
                    pop.unparent();
                }
                pump();
                assert!(target().is_none(), "{key}: the closed menu's target is gone");
                assert_eq!(pending(), want, "{key} stays pending");
            }
        }

        // Removable but not kind "drive": the pre-existing Eject row still follows `removable`,
        // but it carries no guide name, so the pending request never binds or paints.
        let want = put("drive-eject-stick");
        let mut odd = drive("Stick", true, true);
        odd.kind = "network".into();
        show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), odd, 5.0, 5.0);
        pump();
        let odd_menu = popovers_of(sidebar_row.upcast_ref()).into_iter().find(|p| !p.has_css_class("wf-guide-label")).expect("wrong-kind menu");
        let odd_eject = button_labelled(odd_menu.upcast_ref(), "Eject").expect("Eject follows removable");
        assert_ne!(odd_eject.widget_name(), "drive-eject-stick", "no guide name on a wrong-kind Eject row");
        assert!(target().is_none(), "the wrong-kind menu never binds the request");
        assert!(by_name("drive-eject-stick").is_empty());
        assert!(
            popovers_of(ui.window.upcast_ref()).iter().all(|p| !p.has_css_class("wf-guide-label")),
            "nothing painted for a wrong-kind Eject row"
        );
        assert_eq!(pending(), want, "still pending");
        drop((odd_eject, odd_menu));
        for pop in popovers_of(sidebar_row.upcast_ref()) {
            pop.popdown();
            pop.unparent();
        }
        pump();

        // Two live targets with the exact name fail closed (the generic duplicate-name
        // regression is D-52's `no_visual("duplicate name")`; this is the Eject-specific case).
        show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), drive("Stick", true, true), 5.0, 5.0);
        pump();
        let menu = popovers_of(sidebar_row.upcast_ref()).into_iter().find(|p| !p.has_css_class("wf-guide-label")).expect("stick menu");
        assert!(target().is_some(), "one Eject row binds");
        button_labelled(menu.upcast_ref(), "Open").expect("Open row").set_widget_name("drive-eject-stick");
        let want = put("drive-eject-stick");
        assert!(target().is_none(), "a second live target of the same name fails closed");
        assert_eq!(pending(), want, "the fresh request stays pending");
        drop(menu);
        for pop in popovers_of(sidebar_row.upcast_ref()) {
            pop.popdown();
            pop.unparent();
        }
        pump();

        ui.window.destroy();
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[cfg(feature = "guide")]
    fn guide_target_names_are_exact_d50() {
        let registry: serde_json::Value = serde_json::from_str(include_str!("guide-names.json")).unwrap();
        let keys: Vec<String> = registry.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 15);
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d50-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::env::set_var("HOME", &home);
        std::env::set_var("WF_GUIDE_FILE", root.join("none/wayfinder.json"));
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d50test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let named = |root: &gtk::Widget| -> Vec<String> {
            let mut v: Vec<String> = all_widgets(root)
                .iter()
                .map(|w| w.widget_name().to_string())
                .filter(|n| keys.contains(n))
                .collect();
            v.sort();
            v
        };
        // Static targets: the real toolbar buttons, found by tooltip.
        let toolbar_button = |tip: &str| -> Vec<gtk::Button> {
            all_widgets(ui.window.upcast_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .filter(|b| b.tooltip_text().as_deref() == Some(tip))
                .collect()
        };
        let twin = icon_button("edit-undo-symbolic", "twin");
        for (tip, key) in [("Terminal here", "terminal-here-folder"), ("Move to Trash", "trash-and-restore-file")] {
            let found = toolbar_button(tip);
            assert_eq!(found.len(), 1, "one command-bar button for {tip}");
            let b = &found[0];
            assert_eq!(b.widget_name(), key);
            assert!(b.has_css_class("wf-icon-button"), "{tip} is the real toolbar button");
            assert_eq!((b.is_focusable(), b.can_target()), (twin.is_focusable(), twin.can_target()));
            assert_eq!(b.width(), toolbar_button("Copy")[0].width(), "naming moved nothing");
        }
        assert_eq!(named(ui.window.upcast_ref()), vec!["terminal-here-folder", "trash-and-restore-file"]);

        // Open With: the real connected opener in the item menu.
        let has_right_click = |w: &gtk::Widget| {
            let c = w.observe_controllers();
            (0..c.n_items()).any(|i| {
                c.item(i).and_then(|o| o.downcast::<gtk::GestureClick>().ok()).is_some_and(|g| g.button() == 3)
            })
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        show_item_context_menu(&row, state.clone(), ui.clone(), entry, 5.0, 5.0);
        pump();
        let item_menu = popovers_of(&row).pop().expect("item menu");
        let open_with = button_labelled(item_menu.upcast_ref(), "Open With...").expect("Open With row");
        assert_eq!(open_with.widget_name(), "open-with-other-app");
        assert_eq!(named(item_menu.upcast_ref()), vec!["item-menu-cap-d56", "item-menu-full-d60", "open-with-other-app"]);
        assert!(open_with.is_focusable() == twin.is_focusable() && open_with.can_target());
        item_menu.popdown();

        // Drive menus, synthetic drives only.
        let sidebar_row = all_widgets(ui.sidebar.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
            .expect("a sidebar row");
        let drive = |name: &str, kind: &str, removable: bool, mounted: bool| Drive {
            name: name.to_string(),
            path: format!("synthetic:{name}"),
            mountpoint: if mounted { "/nonexistent/synthetic".into() } else { String::new() },
            size: 1,
            fstype: "ext4".into(),
            kind: kind.into(),
            removable,
            space: None,
        };
        let menu_for = |d: Drive| -> (gtk::Popover, Vec<String>) {
            show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), d, 5.0, 5.0);
            pump();
            let pop = popovers_of(sidebar_row.upcast_ref()).pop().expect("drive menu");
            let names = named(pop.upcast_ref());
            (pop, names)
        };
        let crucial = "Launch Box SSD 2TB";
        let cases: Vec<(Drive, Vec<&str>, Option<(&str, &str)>)> = vec![
            (drive(crucial, "drive", false, true), vec!["drive-open-crucial", "drive-unmount-crucial"], Some(("Open", "drive-open-crucial"))),
            (drive(crucial, "drive", false, false), vec!["drive-remount-crucial"], Some(("Mount & Open", "drive-remount-crucial"))),
            (drive("Stick", "drive", true, true), vec!["drive-eject-stick", "drive-open-stick", "drive-unmount-stick"], Some(("Open", "drive-open-stick"))),
            (drive("Stick", "drive", true, false), vec!["drive-eject-stick", "drive-remount-stick"], Some(("Mount & Open", "drive-remount-stick"))),
            (drive("Network", "network", false, true), vec![], None),
            (drive("Removable Network", "network", true, true), vec![], None),
            (drive("Google Drive", "google", false, true), vec![], None),
            (drive("Some Other Disk", "drive", false, true), vec![], None),
            (drive("Some Other Disk", "drive", false, false), vec![], None),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for (d, expect, label) in cases {
            let title = format!("{} mounted={}", d.name, !d.mountpoint.is_empty());
            let (pop, names) = menu_for(d);
            let mut expect: Vec<String> = expect.iter().map(|s| s.to_string()).collect();
            expect.sort();
            assert_eq!(names, expect, "names in the menu for {title}");
            if let Some((text, key)) = label {
                assert_eq!(button_labelled(pop.upcast_ref(), text).unwrap().widget_name(), key, "{title}");
            }
            // Eject exists only on a removable drive; there it is the one real row, named, never duplicated.
            let ejects: Vec<gtk::Widget> = all_widgets(pop.upcast_ref())
                .into_iter()
                .filter(|w| {
                    w.is::<gtk::Button>()
                        && all_widgets(w).iter().any(|c| c.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == "Eject"))
                })
                .collect();
            if title.starts_with("Stick") {
                assert_eq!(ejects.len(), 1, "{title}: exactly one Eject row");
                assert_eq!(ejects[0].widget_name(), "drive-eject-stick", "{title}");
                assert_eq!(names.iter().filter(|n| *n == "drive-eject-stick").count(), 1, "{title}: one named row");
            } else if title.starts_with("Removable Network") {
                // The pre-existing Eject row follows `removable`; only the guide name follows kind too.
                assert_eq!(ejects.len(), 1, "{title}: the ordinary Eject row is still there");
                assert_ne!(ejects[0].widget_name(), "drive-eject-stick", "{title}: but never named");
            } else {
                assert!(ejects.is_empty(), "{title}: no Eject row");
            }
            seen.extend(names);
            pop.popdown();
        }
        seen.extend(named(ui.window.upcast_ref()));
        seen.insert("open-with-other-app".to_string());
        seen.insert("item-menu-cap-d56".to_string());
        seen.insert("item-menu-full-d60".to_string());
        seen.insert("open-file-other-app".to_string());
        seen.insert("drive-space-bars".to_string());
        seen.insert("drive-mounted-again-crucial".to_string());
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), { let mut k = keys.clone(); k.sort(); k }, "all fifteen keys, each on its real control");
        for w in all_widgets(sidebar_row.upcast_ref()) {
            assert!(!keys.contains(&w.widget_name().to_string()) || w.is::<gtk::Popover>() || w.is::<gtk::Button>(), "the sidebar row is not named");
        }
        assert!(!keys.contains(&sidebar_row.widget_name().to_string()));

        ui.window.destroy();
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[cfg(feature = "guide")]
    fn guide_watch_promotes_from_missing_directory_d49(app: &adw::Application, root: &std::path::Path) {
        let base = root.join("d49");
        std::fs::create_dir_all(&base).unwrap();
        let guide_dir = base.join("a/b/guide");
        let file = guide_dir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        build_ui(app, Some(root.join("home").to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        let pending = |s: &Rc<RefCell<AppState>>| s.borrow().guide_pending.clone();
        let live = |s: &Rc<RefCell<AppState>>| {
            s.borrow().guide_monitor.as_ref().is_some_and(|m| !m.is_cancelled())
        };
        assert!(!base.join("a").exists(), "opening the window created a directory");
        assert_eq!(state.borrow().guide_watch_dir.as_deref(), Some(base.as_path()));
        assert!(live(&state));
        assert_eq!(pending(&state), GuideRequest::Clear);
        let first = state.borrow().guide_monitor.clone().unwrap();

        // Siblings at the watched level change nothing.
        std::fs::create_dir(base.join("elsewhere")).unwrap();
        pump();
        assert_eq!(state.borrow().guide_watch_dir.as_deref(), Some(base.as_path()));
        assert!(!first.is_cancelled());

        // Checkrun: create the directories, then write atomically at once.
        std::fs::create_dir_all(&guide_dir).unwrap();
        let tmp = guide_dir.join("wayfinder.json.tmp");
        std::fs::write(&tmp, r#"{"highlight":"drive-remount-stick","label":"Remount"}"#).unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        let want = GuideRequest::Known { name: "drive-remount-stick".into(), label: "Remount".into() };
        wait_for("promotion and first request", || {
            pending(&state) == want && state.borrow().guide_watch_dir.as_deref() == Some(guide_dir.as_path())
        });
        assert!(first.is_cancelled(), "the old monitor was replaced");
        assert!(live(&state));

        // After promotion a sibling changes nothing and the watch stays.
        std::fs::write(guide_dir.join("other.json"), r#"{"highlight":"","label":""}"#).unwrap();
        pump();
        assert_eq!(pending(&state), want);
        assert_eq!(state.borrow().guide_watch_dir.as_deref(), Some(guide_dir.as_path()));

        let last = state.borrow().guide_monitor.clone().unwrap();
        ui.window.close();
        pump();
        assert!(last.is_cancelled() && state.borrow().guide_monitor.is_none());
    }

    #[cfg(feature = "guide")]
    fn guide_unknown_response_is_atomic_d58() {
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d58-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let gdir = root.join("guide");
        for d in [&home, &dir, &gdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::env::set_var("HOME", &home);
        let file = gdir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d58test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS.with(|w| w.borrow().windows().pop()).expect("window");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();
        let pending = |s: &Rc<RefCell<AppState>>| s.borrow().guide_pending.clone();
        let seen = GUIDE_RESPONSE_TEMPS.with(|t| t.borrow().len());
        let temps = || GUIDE_RESPONSE_TEMPS.with(|t| t.borrow()[seen..].to_vec());
        let tree = || all_widgets(ui.window.upcast_ref());
        let rings = || guide_marked(tree()).len();
        let labels = || {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-label")).count()
        };
        let siblings = || -> Vec<String> {
            let mut n: Vec<String> =
                std::fs::read_dir(&gdir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
            n.sort();
            n
        };

        // A painted known target, then an unknown name with JSON-significant characters.
        std::fs::write(&file, r#"{"highlight":"terminal-here-folder","label":"Here"}"#).unwrap();
        wait_for("painted", || state.borrow().guide_visual.is_some() && rings() == 1);
        assert_eq!(temps().len(), 0, "a known request never writes");
        let name = "we\"ird \\ na\u{e9}me\n{\"x\":1}";
        let request = serde_json::json!({"highlight": name, "label": "L"}).to_string();
        let tmp = gdir.join("wayfinder.json.tmp");
        std::fs::write(&tmp, request).unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        wait_for("response written and settled", || {
            pending(&state) == GuideRequest::Clear
                && std::fs::read_to_string(&file).is_ok_and(|t| t.contains("unknown"))
        });
        for _ in 0..20 {
            pump();
        }
        assert!(state.borrow().guide_visual.is_none() && state.borrow().guide_target.is_none(), "visual and target dropped");
        assert_eq!((rings(), labels()), (0, 0), "ring and label removed");
        let text = std::fs::read_to_string(&file).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("the response is whole JSON");
        let obj = value.as_object().expect("an object");
        assert_eq!(obj.len(), 1, "only one field: {text}");
        assert_eq!(obj["unknown"].as_str(), Some(name), "byte-exact original name");
        assert_eq!(temps().len(), 1, "one response write, then stable: {:?}", temps());
        assert_eq!(temps()[0].parent(), Some(gdir.as_path()), "temp is a same-directory sibling");
        assert_eq!(siblings(), vec!["wayfinder.json".to_string()], "no temp sibling remains");
        assert_eq!(pending(&state), GuideRequest::Clear);

        // Clear, malformed and missing never write.
        for text in [r#"{"highlight":"","label":""}"#, "{not json", r#"{"unknown":"x"}"#] {
            std::fs::write(&file, text).unwrap();
            wait_for("clear", || pending(&state) == GuideRequest::Clear);
            for _ in 0..10 {
                pump();
            }
        }
        std::fs::remove_file(&file).unwrap();
        for _ in 0..10 {
            pump();
        }
        assert_eq!(temps().len(), 1, "no path but Unknown writes");

        // Two live windows on the same file: no temp collision, both settle to Clear.
        let (state2, ui2) = {
            build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
            WINDOWS.with(|w| w.borrow().windows().pop()).expect("second window")
        };
        assert!(!Rc::ptr_eq(&state, &state2));
        let before = temps().len();
        std::fs::write(&file, r#"{"highlight":"no-such-control","label":"Hmm"}"#).unwrap();
        wait_for("both clear", || pending(&state) == GuideRequest::Clear && pending(&state2) == GuideRequest::Clear);
        for _ in 0..40 {
            pump();
        }
        let after = temps();
        let n = after.len() - before;
        assert!((1..=2).contains(&n), "one or two identical responses, got {n}");
        let unique: std::collections::HashSet<_> = after.iter().collect();
        assert_eq!(unique.len(), after.len(), "no temp name was reused");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"unknown":"no-such-control"}"#);
        assert_eq!(siblings(), vec!["wayfinder.json".to_string()]);
        for _ in 0..40 {
            pump();
        }
        assert_eq!(temps().len(), after.len(), "no further writes, no loop");
        assert_eq!((pending(&state), pending(&state2)), (GuideRequest::Clear, GuideRequest::Clear));

        // A missing guide directory is not created and leaves no temp.
        let gone = root.join("absent").join("wayfinder.json");
        guide_write_unknown(&gone, "x");
        assert!(!root.join("absent").exists(), "the guide directory is never created");

        ui2.window.destroy();
        ui.window.destroy();
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[cfg(feature = "guide")]
    fn guide_request_transport_d48() {
        let old_env = std::env::var("WF_GUIDE_FILE").ok();
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d48-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("guide");
        for d in [&home, &dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::env::set_var("HOME", &home);
        let file = dir.join("wayfinder.json");
        std::env::set_var("WF_GUIDE_FILE", &file);
        assert_eq!(guide_file_path(), file);
        std::env::set_var("WF_GUIDE_FILE", "");
        assert_eq!(guide_file_path(), home.join(".local/share/crew/guide/wayfinder.json"));
        std::env::set_var("WF_GUIDE_FILE", &file);

        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d48test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        let open_window = || {
            build_ui(&app, Some(home.to_string_lossy().to_string()), None);
            WINDOWS.with(|w| w.borrow().windows().pop()).expect("the test window is registered")
        };
        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        let (state, ui) = open_window();
        let pending = |s: &Rc<RefCell<AppState>>| s.borrow().guide_pending.clone();
        assert_eq!(pending(&state), GuideRequest::Clear, "a missing file is clear");
        assert!(state.borrow().guide_monitor.is_some());

        std::fs::write(&file, r#"{"highlight":"drive-open-crucial","label":"Open it"}"#).unwrap();
        wait_for("direct creation", || pending(&state) == known("drive-open-crucial", "Open it"));

        let tmp = dir.join("wayfinder.json.tmp");
        std::fs::write(&tmp, r#"{"highlight":"trash-and-restore-file","label":"Trash"}"#).unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        wait_for("atomic replace", || pending(&state) == known("trash-and-restore-file", "Trash"));

        std::fs::write(dir.join("other.json"), r#"{"highlight":"","label":""}"#).unwrap();
        pump();
        assert_eq!(pending(&state), known("trash-and-restore-file", "Trash"), "a sibling changed nothing");

        std::fs::write(&file, r#"{"highlight":"","label":"gone"}"#).unwrap();
        wait_for("empty highlight", || pending(&state) == GuideRequest::Clear);
        std::fs::write(&file, r#"{"highlight":"drive-open-stick","label":"x"}"#).unwrap();
        wait_for("reload", || pending(&state) == known("drive-open-stick", "x"));
        std::fs::write(&file, "{not json").unwrap();
        wait_for("malformed", || pending(&state) == GuideRequest::Clear);

        std::fs::write(&file, r#"{"highlight":"no-such-control","label":"Hmm"}"#).unwrap();
        wait_for("unknown settles to clear", || {
            pending(&state) == GuideRequest::Clear
                && std::fs::read_to_string(&file).is_ok_and(|t| t == r#"{"unknown":"no-such-control"}"#)
        });
        pump();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            r#"{"unknown":"no-such-control"}"#,
            "the app answers an unknown name with the locked response (D-58)"
        );

        // A second live window keeps its own single, newest request.
        let (state2, ui2) = open_window();
        assert!(!Rc::ptr_eq(&state, &state2));
        assert_eq!(pending(&state2), GuideRequest::Clear, "the initial read takes the response as clear");
        std::fs::write(&file, r#"{"highlight":"open-with-other-app","label":"Newest"}"#).unwrap();
        wait_for("both windows update", || {
            pending(&state) == known("open-with-other-app", "Newest") && pending(&state2) == known("open-with-other-app", "Newest")
        });

        // Closing one window releases its watcher; the other keeps working.
        let monitor = state2.borrow().guide_monitor.clone().expect("second monitor");
        ui2.window.close();
        pump();
        assert!(monitor.is_cancelled() && state2.borrow().guide_monitor.is_none(), "closed window kept its watcher");
        std::fs::write(&file, r#"{"highlight":"drive-open-crucial","label":"After"}"#).unwrap();
        wait_for("first window still live", || pending(&state) == known("drive-open-crucial", "After"));
        assert_eq!(pending(&state2), known("open-with-other-app", "Newest"), "a closed window is not updated");

        ui.window.destroy();
        guide_watch_promotes_from_missing_directory_d49(&app, &root);
        match old_env {
            Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
            None => std::env::remove_var("WF_GUIDE_FILE"),
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn preview_pane_toggle_is_one_safe_route_d30() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d30-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "AAA-preview-text").unwrap();
        std::fs::write(dir.join("b.txt"), "BBB-preview-text").unwrap();
        std::env::set_var("HOME", &home);
        let dir_str = dir.to_string_lossy().to_string();
        let open = |id: &str| {
            let app = adw::Application::builder()
                .application_id(id)
                .flags(gio::ApplicationFlags::NON_UNIQUE)
                .build();
            app.register(gio::Cancellable::NONE).expect("register test application");
            build_ui(&app, Some(dir_str.clone()), None);
            let found = WINDOWS
                .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
                .expect("the test window is registered");
            (app, found.0, found.1)
        };
        let (_app, state, ui) = open("com.kadaken.wayfinder.d30test");
        wait_for("listing", || state.borrow().visible_entries.len() == 2);
        let path = |n: &str| dir.join(n).to_string_lossy().to_string();
        let invariants = || {
            let s = state.borrow();
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            (
                names,
                s.clipboard.is_none(),
                s.undo_stack.len(),
                (s.tabs.tabs[0].history.clone(), s.tabs.tabs[0].index),
                (ui.back.is_sensitive(), ui.forward.is_sensitive()),
                (view_mode_id(s.view_mode), s.sort_key, s.sort_ascending, s.filter_text.clone()),
                format!("{:?}", load_config()),
            )
        };
        let before = invariants();

        // reachable, active by default, pane visible
        assert_eq!(ui.preview_toggle.tooltip_text().as_deref(), Some("Preview pane"));
        match ui.preview_toggle.icon_name() {
            Some(name) => assert!(
                gtk::IconTheme::for_display(&gdk::Display::default().unwrap()).has_icon(name.as_str()),
                "installed icon {name}"
            ),
            None => assert_eq!(ui.preview_toggle.label().as_deref(), Some("\u{25AD}")),
        }
        assert!(ui.preview_toggle.is_active() && ui.preview_toggle.is_mapped());
        let mut in_bar = false;
        let mut w = ui.preview_toggle.parent();
        while let Some(p) = w {
            in_bar |= p.has_css_class("wf-commandbar");
            w = p.parent();
        }
        assert!(in_bar, "the toggle sits in the command bar");
        assert!(ui.preview_scroll.is_visible() && ui.preview_scroll.is_mapped());
        assert!(ui.preview_scroll.parent().as_ref() == Some(ui.file_preview.upcast_ref()));

        select_entry_by_path(&state, &ui, &path("a.txt"));
        wait_for("a preview", || ui.preview.label().contains("AAA-preview-text"));

        // a valid non-default divider position
        ui.window.set_size_request(1400, 600);
        wait_for("paned width", || ui.file_preview.width() > 900);
        // The window may still be growing toward the requested size on a screen that cannot
        // show it whole; a divider chosen from a transient width is rescaled by the resize.
        // Settle first: the paned width must be unchanged across consecutive main-loop turns.
        let last = std::cell::Cell::new(-1);
        let stable = std::cell::Cell::new(0);
        wait_for("paned width settles", || {
            let now = ui.file_preview.width();
            stable.set(if now == last.get() { stable.get() + 1 } else { 0 });
            last.set(now);
            pump();
            stable.get() >= 5
        });
        let width = ui.file_preview.width();
        let default_pos = ui.file_preview.position();
        let custom = default_pos + (width - 190 - default_pos) / 2;
        assert!(custom > default_pos + 10, "room to move the divider: {default_pos} {width}");
        ui.file_preview.set_position(custom);
        pump();
        assert_eq!(ui.file_preview.position(), custom);

        // off: pane hidden, resources released, files side reclaims the width
        ui.preview_toggle.set_active(false);
        wait_for("pane hidden", || !ui.preview_scroll.is_mapped());
        assert!(!ui.preview_scroll.is_visible() && !ui.preview_toggle.is_active());
        assert!(ui.preview_media_picture.paintable().is_none());
        assert!(ui.preview_media_controls.media_stream().is_none());
        assert!(ui.preview_picture.file().is_none());
        assert_eq!(ui.preview.label(), "Select a file");
        let files = ui.file_preview.start_child().unwrap();
        wait_for("files side reclaims", || files.width() >= ui.file_preview.width() - 2);
        assert_eq!(ui.preview_saved_position.get(), custom);

        // select another row while off: selection/status change, no preview work
        select_entry_by_path(&state, &ui, &path("b.txt"));
        assert_eq!(selected_paths(&state, &ui), vec![path("b.txt")]);
        assert!(ui.status.label().contains("1 selected"), "{}", ui.status.label());
        pump();
        assert_eq!(ui.preview.label(), "Select a file", "no preview rendered while off");
        assert!(ui.preview_media_picture.paintable().is_none());
        ui.preview_toggle.set_active(false);
        assert!(!ui.preview_scroll.is_visible(), "re-selecting off is a no-op");

        // on: pane returns, current selection renders, divider restored
        ui.preview_toggle.set_active(true);
        wait_for("pane shown", || ui.preview_scroll.is_mapped());
        wait_for("b preview", || ui.preview.label().contains("BBB-preview-text"));
        assert!(ui.preview_toggle.is_active());
        wait_for("divider restored", || ui.file_preview.position() == custom);

        // repeated cycles do not drift
        for _ in 0..2 {
            ui.preview_toggle.set_active(false);
            wait_for("hidden again", || !ui.preview_scroll.is_mapped());
            assert_eq!(ui.preview_saved_position.get(), custom);
            ui.preview_toggle.set_active(true);
            wait_for("shown again", || ui.preview_scroll.is_mapped());
            wait_for("no drift", || ui.file_preview.position() == custom);
            assert!(ui.preview.label().contains("BBB-preview-text"));
        }
        assert_eq!(
            all_widgets(ui.file_preview.upcast_ref())
                .iter()
                .filter(|w| w.has_css_class("wf-preview"))
                .count(),
            1,
            "one preview pane"
        );
        assert_eq!(invariants(), before);

        // a new window defaults to visible and active
        let (_app2, _state2, ui2) = open("com.kadaken.wayfinder.d30test2");
        assert!(ui2.preview_toggle.is_active() && ui2.preview_scroll.is_visible());
        ui2.window.destroy();
        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn job_result_survives_its_own_refresh_d32() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d32-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let other = root.join("other");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(other.join("o.txt"), "O").unwrap();
        std::env::set_var("HOME", &home);
        let dir_str = dir.to_string_lossy().to_string();
        let other_str = other.to_string_lossy().to_string();
        let missing = root.join("missing").to_string_lossy().to_string();
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d32test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        wait_for("first status", || ui.status.label() == "1 item shown");

        // A successful job: busy label, its own same-folder refresh, then its result stays.
        run_job(
            state.clone(),
            ui.clone(),
            "Working D32".to_string(),
            || {
                std::thread::sleep(Duration::from_millis(300));
                Ok(JobOutcome::message("D32 job finished"))
            },
            |_, _, _| {},
        );
        assert_eq!(ui.status.label(), "Working D32");
        wait_for("job result", || ui.status.label() == "D32 job finished");
        assert!(state.borrow().listing_cancel.is_none(), "the refresh had already completed");
        assert_eq!(state.borrow().visible_entries.len(), 1);
        pump();
        pump();
        assert_eq!(ui.status.label(), "D32 job finished", "the result survives its own refresh");

        // Ordinary navigation replaces it with the normal listing status.
        navigate(&state, other_str.clone());
        wait_for("ordinary status", || ui.status.label() == "1 item shown");
        assert_eq!(state.borrow().cwd, other_str);

        // A failed listing shows its error, never the queued success.
        run_job(
            state.clone(),
            ui.clone(),
            "Working ghost".to_string(),
            {
                let missing = missing.clone();
                move || Ok(JobOutcome::navigate("D32 ghost success", missing))
            },
            |_, _, _| {},
        );
        wait_for("listing error", || ui.status.label().starts_with("Cannot open"));
        pump();
        assert!(ui.status.label().starts_with("Cannot open"), "{}", ui.status.label());
        assert!(!ui.status.label().contains("D32 ghost success"));

        // An overtaken request cannot put its old message on the newer path.
        navigate_to_with_message(&state, other_str.clone(), NavMode::Keep, Some("D32 stale".to_string()));
        navigate(&state, dir_str.clone());
        wait_for("newer path", || {
            state.borrow().cwd == dir_str && ui.status.label() == "1 item shown"
        });
        pump();
        pump();
        assert_eq!(state.borrow().cwd, dir_str);
        assert_eq!(ui.status.label(), "1 item shown", "the stale message never appears");

        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn folder_activation_waits_for_gtk_to_finish_d33() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d33-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let sub = dir.join("sub");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), "I").unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        std::env::set_var("HOME", &home);
        let dir_str = dir.to_string_lossy().to_string();
        let sub_str = sub.to_string_lossy().to_string();
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d33test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        let back_to_dir = |mode: ViewMode| {
            navigate(&state, dir_str.clone());
            wait_for("listing", || state.borrow().cwd == dir_str && state.borrow().visible_entries.len() == 3);
            set_view_mode(&state, &ui, mode);
            pump();
        };
        let position_of = |name: &str| {
            state.borrow().visible_entries.iter().position(|e| e.name == name).expect(name)
        };

        // Details, then Compact: the real list activation action, armed by the real gate.
        for mode in [ViewMode::Details, ViewMode::Compact] {
            back_to_dir(mode);
            let pos = position_of("sub");
            // a bare activation opens nothing, even after the deferred turn
            ui.list.activate_action("list.activate-item", Some(&(pos as u32).to_variant())).unwrap();
            pump();
            assert_eq!(state.borrow().cwd, dir_str, "a bare activation opens nothing");
            {
                let gate = state.borrow().activation_gate.clone();
                gate.borrow_mut().press(&sub_str, std::time::Instant::now(), true);
                gate.borrow_mut().press(&sub_str, std::time::Instant::now(), true);
            }
            let generation = state.borrow().listing_generation;
            let rows = ui.list_store.n_items();
            ui.list.activate_action("list.activate-item", Some(&(pos as u32).to_variant())).unwrap();
            assert_eq!(state.borrow().listing_generation, generation, "no navigation began inside the signal");
            assert_eq!(ui.list_store.n_items(), rows, "the activated row is still alive when the signal returns");
            wait_for("deferred open", || state.borrow().cwd == sub_str);
            wait_for("sub listing", || state.borrow().visible_entries.len() == 1);
        }

        // Grid: the real child activation.
        back_to_dir(ViewMode::Grid);
        let pos = position_of("sub");
        let child = ui.grid.child_at_index(pos as i32).expect("grid child");
        let generation = state.borrow().listing_generation;
        child.emit_by_name::<()>("activate", &[]);
        assert_eq!(state.borrow().listing_generation, generation, "no navigation began inside the grid signal");
        assert!(child.parent().is_some(), "the activated grid child is still alive when the signal returns");
        wait_for("deferred grid open", || state.borrow().cwd == sub_str);
        wait_for("sub listing", || state.borrow().visible_entries.len() == 1);

        // Identity survives deferral: both files vanish so nothing can launch, then the
        // entries are reversed; only the captured entry (b.txt) may be refused.
        back_to_dir(ViewMode::Grid);
        let pos = position_of("b.txt");
        let child = ui.grid.child_at_index(pos as i32).expect("grid child");
        child.emit_by_name::<()>("activate", &[]);
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        std::fs::remove_file(dir.join("b.txt")).unwrap();
        state.borrow_mut().visible_entries.reverse();
        wait_for("captured entry refused", || ui.status.label() == "b.txt no longer exists");

        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// D-34: navigating to a different folder clears the focused row; focus must
    /// already sit on the surviving view root, and only when it was in the file pane.
    fn item_right_click_menu_opens_bounded_and_stays_d41() {
        let root = crate::native_core::scratch_root().join(format!("wf-d41-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("with-me.txt"), "hello").unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d41test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        let dir_str = dir.to_string_lossy().to_string();
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        // The row's own right-click handler, the same route a real click takes.
        let has_right_click = |w: &gtk::Widget| {
            let controllers = w.observe_controllers();
            (0..controllers.n_items()).any(|i| {
                controllers
                    .item(i)
                    .and_then(|o| o.downcast::<gtk::GestureClick>().ok())
                    .is_some_and(|g| g.button() == 3)
            })
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row carries the right-click gesture");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        show_item_context_menu(&row, state.clone(), ui.clone(), entry, 5.0, 5.0);
        // Several frames: GTK pops an oversized popover down on its first layout.
        pump();
        pump();

        let popover = popovers_of(&row).pop().expect("the item menu popover");
        let monitor_h = gdk::Display::default()
            .and_then(|d| d.monitors().item(0))
            .and_then(|m| m.downcast::<gdk::Monitor>().ok())
            .map(|m| m.geometry().height())
            .expect("a monitor");
        assert!(popover.is_visible() && popover.is_mapped(), "the item menu stays open after its first layout");
        let scroller = all_widgets(popover.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ScrolledWindow>().ok())
            .expect("the item menu scroller");
        let adj = scroller.vadjustment();
        let range = adj.upper() - adj.page_size();
        let actions = [
            "Open With...", "Copy Paths", "Compress to ZIP", "Move to Trash", "Delete Permanently",
            "Properties", "Open Terminal Here",
        ];
        for label in actions {
            assert!(button_labelled(popover.upcast_ref(), label).is_some(), "the real menu keeps its action {label}");
        }
        let terminal = button_labelled(popover.upcast_ref(), "Open Terminal Here").expect("Open Terminal Here");
        eprintln!("D60 monitor_h={monitor_h} popover_h={} scroller_h={} range={range} scrollbar_mapped={}", popover.height(), scroller.height(), scroller.vscrollbar().is_mapped());
        if monitor_h >= 800 {
            // D-60: a screen with room shows the whole real menu at once, no scrolling.
            assert!(range <= 0.5, "large screen ({monitor_h} px): no scroll range, got {range}");
            assert!(!scroller.vscrollbar().is_mapped(), "large screen: no visible vertical scrollbar");
            let b = terminal.compute_bounds(&scroller).expect("terminal bounds");
            assert!(
                b.y() >= 0.0 && b.y() + b.height() <= scroller.height() as f32 + 0.5,
                "large screen: Open Terminal Here is visible without scrolling"
            );
            assert!(popover.height() > 0 && popover.height() <= monitor_h, "large screen: outer menu {} px", popover.height());
        } else if monitor_h <= 480 {
            assert!(popover.height() > 0 && popover.height() <= monitor_h, "tiny screen: outer menu {} px on {monitor_h}", popover.height());
            assert!(range > 1.0, "tiny screen ({monitor_h} px): the menu scrolls, range {range}");
            adj.set_value(adj.upper() - adj.page_size());
            pump();
            let b = terminal.compute_bounds(&scroller).expect("terminal bounds");
            assert!(
                b.y() >= 0.0 && b.y() + b.height() <= scroller.height() as f32 + 0.5,
                "tiny screen: the bottom action is reachable by scrolling"
            );
        }
        // The sizing policy itself.
        assert_eq!(item_menu_content_cap(490, 26, 1080), 490, "fits: the whole content");
        assert_eq!(item_menu_content_cap(490, 26, 480), 480 - ITEM_MENU_EDGE_ALLOWANCE - 26, "does not fit: bounded");
        assert_eq!(item_menu_content_cap(490, 26, 10), 1, "never below one pixel");
        assert!(ui.window.is_sensitive() && ui.window.is_visible(), "the file window stays usable");

        popover.popdown();
        pump();
        assert!(!popover.is_mapped());
        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// D-57: Terminal Here and Open With start their child through one path that carries the
    /// initiating window's launch context. Nothing real is started; the seam records the
    /// decision. Where the child lands on screen is not asserted: that needs the installed gate.
    fn child_apps_launch_with_the_window_context_d57() {
        let root = crate::native_core::scratch_root().join(format!("wf-d57-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir with space");
        for d in [&home, &dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("with-me.txt"), "hello").unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        TEST_OPEN_WITH_APPS.with(|a| {
            *a.borrow_mut() =
                Some(vec![native_core::AppEntry { id: "fake-editor-a.desktop".into(), name: "Fake Editor A".into() }])
        });
        TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().clear());
        TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().clear());
        TEST_CHILD_MISSING.with(|m| m.borrow_mut().clear());
        TEST_CHILD_FAILS.with(|f| f.set(false));
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d57test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();
        let dir_str = dir.to_string_lossy().to_string();
        let attempts = || TEST_CHILD_ATTEMPTS.with(|a| a.borrow().clone());

        // Open With: the chosen app and file, one launch, context from this window's display.
        let file = dir.join("with-me.txt").to_string_lossy().to_string();
        launch_open_with(&ui, "fake-editor-a.desktop", &file).expect("open with succeeds");
        let got = attempts();
        assert_eq!(got.len(), 1, "one launch");
        assert_eq!(got[0].target, "/test/fake-editor-a.desktop");
        assert_eq!(got[0].subject, file);
        assert!(got[0].context_from_display, "Open With carries the initiating window's display context");
        assert_eq!(TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow().clone()), vec![("fake-editor-a.desktop".to_string(), file.clone())]);

        // Terminal Here: Konsole first, exact directory, same context, one launch.
        TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().clear());
        launch_terminal(&ui, &dir_str);
        let got = attempts();
        assert_eq!(got.len(), 1, "one launch");
        assert_eq!(got[0].target, "konsole");
        assert_eq!(got[0].subject, dir_str);
        assert_eq!(got[0].argv, vec!["konsole".to_string(), "--workdir".to_string(), dir_str.clone()]);
        assert!(got[0].context_from_display, "Terminal Here carries the same window context");

        // Missing candidates fall through in order; the working directory stays exact for each.
        TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().clear());
        TEST_CHILD_MISSING.with(|m| *m.borrow_mut() = vec!["konsole".into(), "kgx".into(), "gnome-terminal".into()]);
        launch_terminal(&ui, &dir_str);
        let names: Vec<String> = attempts().iter().map(|r| r.target.clone()).collect();
        assert_eq!(names, ["konsole", "kgx", "gnome-terminal", "x-terminal-emulator"]);
        let last = attempts().pop().unwrap();
        assert_eq!(last.argv[..4], ["sh", "-c", "cd \"$1\" && exec x-terminal-emulator", "sh"]);
        assert_eq!(last.argv[4], dir_str);
        assert!(!ui.status.label().contains("failed"), "a fallthrough that succeeds reports nothing");

        // Every candidate missing: reported once, in order, none started.
        TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().clear());
        TEST_CHILD_MISSING.with(|m| {
            *m.borrow_mut() =
                ["konsole", "kgx", "gnome-terminal", "x-terminal-emulator", "xterm"].map(String::from).to_vec()
        });
        ui.status.set_label("");
        launch_terminal(&ui, &dir_str);
        assert_eq!(attempts().len(), 5);
        assert_eq!(ui.status.label(), "Terminal failed: no supported terminal found");

        // A launch that fails is reported once and no later candidate is tried.
        TEST_CHILD_ATTEMPTS.with(|a| a.borrow_mut().clear());
        TEST_CHILD_MISSING.with(|m| m.borrow_mut().clear());
        TEST_CHILD_FAILS.with(|f| f.set(true));
        ui.status.set_label("");
        launch_terminal(&ui, &dir_str);
        assert_eq!(attempts().len(), 1, "a failed launch stops the walk");
        assert_eq!(ui.status.label(), "Terminal failed: test launch failure");

        TEST_CHILD_FAILS.with(|f| f.set(false));
        TEST_OPEN_WITH_APPS.with(|a| *a.borrow_mut() = None);
        ui.window.destroy();
        pump();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// D-54: representative actions fire exactly once and a closed menu no longer stays under its
    /// anchor. GTK may keep the closed root shell and its buttons, so nothing here asserts that
    /// a root or child widget is released.
    fn context_menu_actions_fire_once_and_leave_no_popover_d54() {
        let root = crate::native_core::scratch_root().join(format!("wf-d54-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let mount = root.join("mount");
        for d in [&home, &dir, &mount] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("with-me.txt"), "hello").unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        TEST_OPEN_WITH_APPS.with(|a| {
            *a.borrow_mut() =
                Some(vec![native_core::AppEntry { id: "fake-editor-a.desktop".into(), name: "Fake Editor A".into() }])
        });
        TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().clear());
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d54test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let has_right_click = |w: &gtk::Widget| {
            let c = w.observe_controllers();
            (0..c.n_items()).any(|i| c.item(i).and_then(|o| o.downcast::<gtk::GestureClick>().ok()).is_some_and(|g| g.button() == 3))
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row carries the right-click gesture");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        let sidebar_row = all_widgets(ui.sidebar.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
            .expect("a sidebar row");
        let drive = Drive {
            name: "Synthetic".into(),
            path: "/dev/synthetic-d54".into(),
            mountpoint: mount.to_string_lossy().to_string(),
            size: 1,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable: true,
            space: None,
        };

        for i in 0..3 {
            // File menu, closed directly or through its Open With side menu.
            show_item_context_menu(&row, state.clone(), ui.clone(), entry.clone(), 5.0, 5.0);
            pump();
            {
                let menu = popovers_of(&row).pop().expect("the item menu");
                if i % 2 == 0 {
                    menu.popdown();
                } else {
                    let open_with = button_labelled(menu.upcast_ref(), "Open With...").expect("Open With");
                    open_with.emit_clicked();
                    pump();
                    let side = popovers_of(open_with.upcast_ref()).pop().expect("the side menu");
                    button_labelled(side.upcast_ref(), "Fake Editor A").expect("editor row").emit_clicked();
                }
            }
            pump();
            assert!(popovers_of(&row).is_empty(), "no closed popover is left under the row");

            // Drive menu.
            show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), drive.clone(), 5.0, 5.0);
            pump();
            popovers_of(sidebar_row.upcast_ref()).pop().expect("the drive menu").popdown();
            pump();
            assert!(popovers_of(sidebar_row.upcast_ref()).is_empty(), "no closed popover is left under the sidebar row");

            // Background menu with its New side menu.
            show_background_context_menu(&ui.list, state.clone(), ui.clone(), 10.0, 10.0);
            pump();
            {
                let menu = popovers_of(ui.list.upcast_ref()).pop().expect("the background menu");
                let new = button_labelled(menu.upcast_ref(), "New").expect("New");
                new.emit_clicked();
                pump();
                popovers_of(new.upcast_ref()).pop().expect("the New side menu").popdown();
                menu.popdown();
            }
            pump();
            assert!(popovers_of(ui.list.upcast_ref()).is_empty(), "no closed popover is left under the list");
        }

        TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().clear());
        // Real actions still fire exactly once.
        show_item_context_menu(&row, state.clone(), ui.clone(), entry.clone(), 5.0, 5.0);
        pump();
        let menu = popovers_of(&row).pop().expect("the item menu");
        let open_with = button_labelled(menu.upcast_ref(), "Open With...").unwrap();
        open_with.emit_clicked();
        pump();
        let side = popovers_of(open_with.upcast_ref()).pop().unwrap();
        button_labelled(side.upcast_ref(), "Fake Editor A").unwrap().emit_clicked();
        pump();
        assert_eq!(TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow().len()), 1, "the side-menu action fired exactly once");
        assert!(!menu.is_mapped(), "choosing an app still closes the parent menu");
        drop((menu, open_with, side));
        pump();

        show_item_context_menu(&row, state.clone(), ui.clone(), entry.clone(), 5.0, 5.0);
        pump();
        let menu = popovers_of(&row).pop().unwrap();
        button_labelled(menu.upcast_ref(), "Copy").unwrap().emit_clicked();
        pump();
        assert!(state.borrow().clipboard.is_some(), "Copy still copies");
        assert!(!menu.is_mapped(), "Copy still closes the menu");
        drop(menu);
        pump();

        show_drive_context_menu(&sidebar_row, state.clone(), ui.clone(), drive.clone(), 5.0, 5.0);
        pump();
        let menu = popovers_of(sidebar_row.upcast_ref()).pop().unwrap();
        button_labelled(menu.upcast_ref(), "Open").unwrap().emit_clicked();
        pump();
        assert_eq!(
            state.borrow().tabs.active().path,
            mount.to_string_lossy().to_string(),
            "the drive Open action navigated to the mountpoint"
        );
        assert_eq!(state.borrow().tabs.active().history.len(), 2, "one navigation, one history entry");
        drop(menu);
        pump();

        // A button whose popover is already gone still runs its action once.
        let fired = Rc::new(std::cell::Cell::new(0));
        let popover = gtk::Popover::new();
        let button = context_item("", "Lone", "", &popover, {
            let fired = fired.clone();
            move || fired.set(fired.get() + 1)
        });
        drop(popover);
        button.emit_clicked();
        assert_eq!(fired.get(), 1, "the action runs once even when its popover is gone");

        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn open_with_opens_a_side_menu_not_a_dialog_d42() {
        let root = crate::native_core::scratch_root().join(format!("wf-d42-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("with-me.txt"), "hello").unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        TEST_OPEN_WITH_APPS.with(|a| {
            *a.borrow_mut() = Some(vec![
                native_core::AppEntry { id: "fake-editor-a.desktop".into(), name: "Fake Editor A".into() },
                native_core::AppEntry { id: "fake-editor-b.desktop".into(), name: "Fake Editor B".into() },
            ])
        });
        TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow_mut().clear());
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d42test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        let dir_str = dir.to_string_lossy().to_string();
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        pump();

        let has_right_click = |w: &gtk::Widget| {
            let controllers = w.observe_controllers();
            (0..controllers.n_items()).any(|i| {
                controllers
                    .item(i)
                    .and_then(|o| o.downcast::<gtk::GestureClick>().ok())
                    .is_some_and(|g| g.button() == 3)
            })
        };
        let row = all_widgets(ui.list.upcast_ref())
            .into_iter()
            .find(|w| w.is::<gtk::Box>() && has_right_click(w))
            .expect("a file row carries the right-click gesture");
        ui.list_selection.select_item(0, true);
        update_selection(&state, &ui);
        let entry = state.borrow().visible_entries[0].clone();
        let file_path = entry.path.clone();
        show_item_context_menu(&row, state.clone(), ui.clone(), entry, 5.0, 5.0);
        pump();
        pump();
        let item_menu = popovers_of(&row).pop().expect("the item menu popover");
        assert!(item_menu.is_mapped(), "the item menu is open");

        // The real Open With row, through its connected click route.
        let open_with = button_labelled(item_menu.upcast_ref(), "Open With...").expect("the Open With row");
        open_with.emit_clicked();
        pump();
        pump();

        assert!(
            !gtk::Window::list_toplevels().iter().any(|w| w.is::<gtk::Dialog>() && w.is_visible()),
            "Open With opens no dialog or separate window"
        );
        assert!(item_menu.is_mapped(), "the parent item menu stays open behind the side menu");
        let side = popovers_of(open_with.upcast_ref()).pop().expect("a popover anchored to the Open With row");
        assert!(side.is_visible() && side.is_mapped(), "the side menu is open");
        assert!(side.height() > 0 && side.height() <= ITEM_MENU_MAX_HEIGHT, "the side menu is compact: {} px", side.height());
        assert!(button_labelled(side.upcast_ref(), "Fake Editor A").is_some());
        assert!(button_labelled(side.upcast_ref(), "Fake Editor B").is_some());
        assert!(ui.window.is_sensitive() && ui.window.is_visible(), "the file window stays usable");
        assert!(TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow().is_empty()), "nothing launches before a choice");

        button_labelled(side.upcast_ref(), "Fake Editor A").unwrap().emit_clicked();
        pump();
        pump();
        assert!(!side.is_mapped() && !item_menu.is_mapped(), "choosing an app closes both menus");
        let launches = TEST_OPEN_WITH_LAUNCHES.with(|l| l.borrow().clone());
        assert_eq!(launches, vec![("fake-editor-a.desktop".to_string(), file_path)], "launches exactly once");
        assert_eq!(ui.status.label(), "Opened with Fake Editor A");
        assert!(ui.window.is_sensitive() && ui.window.is_visible());

        TEST_OPEN_WITH_APPS.with(|a| *a.borrow_mut() = None);
        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn navigation_hands_focus_to_the_view_root_d34() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d34-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        let sub = dir.join("sub");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), "I").unwrap();
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        std::env::set_var("HOME", &home);
        let dir_str = dir.to_string_lossy().to_string();
        let sub_str = sub.to_string_lossy().to_string();
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d34test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        let focus = || gtk::prelude::GtkWindowExt::focus(&ui.window);
        let back_to_dir = |mode: ViewMode| {
            navigate(&state, dir_str.clone());
            wait_for("listing", || state.borrow().cwd == dir_str && state.borrow().visible_entries.len() == 3);
            set_view_mode(&state, &ui, mode);
            pump();
        };

        for mode in [ViewMode::Details, ViewMode::Compact, ViewMode::Grid] {
            back_to_dir(mode);
            let (root_widget, rows): (gtk::Widget, Box<dyn Fn() -> u32>) = if mode == ViewMode::Grid {
                let child = ui.grid.child_at_index(0).expect("grid child");
                child.grab_focus();
                (ui.grid.clone().upcast(), Box::new(|| {
                    let mut n = 0;
                    let mut c = ui.grid.first_child();
                    while let Some(w) = c { n += 1; c = w.next_sibling(); }
                    n
                }))
            } else {
                ui.list.grab_focus();
                (ui.list.clone().upcast(), Box::new(|| ui.list_store.n_items()))
            };
            pump();
            assert!(focus_within(&ui.window, &root_widget) && focus().as_ref() != Some(&root_widget), "focus is on a row");
            navigate(&state, sub_str.clone());
            assert_eq!(rows(), 0, "the old rows are removed synchronously");
            assert_eq!(focus(), Some(root_widget.clone()), "the view root owns focus before the next frame");
            wait_for("sub listing", || state.borrow().cwd == sub_str && state.borrow().visible_entries.len() == 1);
            pump();
            assert_eq!(state.borrow().cwd, sub_str);
        }

        // Focus outside the file pane is never taken.
        back_to_dir(ViewMode::Details);
        ui.search.grab_focus();
        pump();
        let outside = focus();
        assert!(outside.is_some() && !focus_within(&ui.window, &ui.list) && !focus_within(&ui.window, &ui.grid));
        navigate(&state, sub_str.clone());
        assert_eq!(focus(), outside, "focus outside the file view is not stolen");
        wait_for("sub listing", || state.borrow().cwd == sub_str && state.borrow().visible_entries.len() == 1);
        pump();

        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// D-35: Clear Recents empties only `recents`, on disk and in every live
    /// window, and no stale window can bring an old entry back.
    /// D-65: the only app-owned settings path is `$HOME/.config/wayfinder/native-config.json`,
    /// independent of the application ID, so an ID change needs no migration.
    fn settings_survive_the_application_id_d65() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d65-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let cwd = root.join("cwd-65");
        let fav = root.join("fav-65");
        let recent = root.join("recent-65");
        for d in [&home, &cwd, &fav, &recent] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::env::set_var("HOME", &home);
        let path = config_path().unwrap();
        assert_eq!(path, home.join(".config").join("wayfinder").join("native-config.json"));
        let fav_s = fav.to_string_lossy().to_string();
        let recent_s = recent.to_string_lossy().to_string();
        update_config_at(&path, |c| {
            add_favorite(&mut c.favorites, &fav_s);
            push_recent(&mut c.recents, &recent_s);
            c.show_hidden = true;
            c.view_mode = "grid".to_string();
        });
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        assert_eq!(app.application_id().as_deref(), Some("com.kadaken.wayfinder"));
        build_ui(&app, Some(cwd.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().windows())
            .into_iter()
            .find(|(_, u)| u.window.application().as_ref() == Some(app.upcast_ref()))
            .expect("window under the new application id");
        {
            let s = state.borrow();
            assert!(s.show_hidden, "sentinel show_hidden was read");
            assert!(s.view_mode == ViewMode::Grid, "sentinel view mode was read");
            assert_eq!(s.favorites.iter().map(|b| b.path.clone()).collect::<Vec<_>>(), vec![fav_s.clone()]);
        }
        assert_eq!(read_config_at(&path).recents.iter().map(|b| b.path.clone()).collect::<Vec<_>>(), vec![recent_s]);
        // No app state lives under an application-ID-named directory.
        assert!(!home.join(".var").exists() && !home.join(".config").join("com.kadaken.wayfinder").exists());
        ui.window.destroy();
        pump();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn clear_recents_is_durable_and_live_d35() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d35-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dirs: Vec<_> = ["recent-one-35", "recent-two-35", "fav-35", "win-a-35", "win-b-35", "new-visit-35"]
            .iter()
            .map(|n| root.join(n))
            .collect();
        for d in &dirs {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(format!("marker-{}.txt", d.file_name().unwrap().to_string_lossy())), "M").unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);
        // Every seeded directory, its entries and their exact bytes.
        let tree = || {
            let mut all = Vec::new();
            for d in &dirs {
                let mut names: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()).collect();
                names.sort();
                all.push((d.clone(), names.iter().map(|n| (n.clone(), std::fs::read(n).unwrap())).collect::<Vec<_>>()));
            }
            all
        };
        let tree_before = tree();
        let p = |i: usize| dirs[i].to_string_lossy().to_string();
        let path = config_path().unwrap();
        update_config_at(&path, |c| {
            push_recent(&mut c.recents, &p(0));
            push_recent(&mut c.recents, &p(1));
            add_favorite(&mut c.favorites, &p(2));
            c.show_hidden = true;
            c.sort_ascending = false;
            c.search_in_files = true;
        });
        let others = || {
            let c = read_config_at(&path);
            serde_json::to_string(&(c.favorites, c.show_hidden, c.view_mode, c.search_in_files, c.sort_key, c.sort_ascending)).unwrap()
        };
        let others_before = others();
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d35test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(p(3)), None);
        build_ui(&app, Some(p(4)), None);
        let mine = |(_, u): &(Rc<RefCell<AppState>>, Ui)| u.window.application().as_ref() == Some(app.upcast_ref());
        let windows: Vec<_> = WINDOWS.with(|w| w.borrow().windows()).into_iter().filter(|w| mine(w)).collect();
        assert_eq!(windows.len(), 2);
        let find = |cwd: &str| {
            windows.iter().find(|(s, _)| s.borrow().cwd == cwd).cloned().expect("window by cwd")
        };
        let (sa, ua) = find(&p(3));
        let (sb, ub) = find(&p(4));
        wait_for("both listings", || {
            sa.borrow().visible_entries.len() == 1 && sb.borrow().visible_entries.len() == 1
        });
        ua.list_selection.select_item(0, true);
        pump();
        let snap = |s: &Rc<RefCell<AppState>>, u: &Ui| {
            let s = s.borrow();
            (s.cwd.clone(), s.tabs.tabs.clone(), selected_indices(u))
        };
        let (snap_a, snap_b) = (snap(&sa, &ua), snap(&sb, &ub));
        assert_eq!(snap_a.2, vec![0]);
        let recent_heading = |u: &Ui| {
            all_widgets(u.sidebar.upcast_ref())
                .iter()
                .any(|w| w.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == "Recent"))
        };
        assert!(recent_heading(&ua) && recent_heading(&ub), "recents shown before clearing");
        let rows_before = all_widgets(ua.sidebar.upcast_ref()).iter().filter(|w| w.is::<gtk::ListBoxRow>()).count();
        let disk_recents = || read_config_at(&path).recents.iter().map(|r| r.path.clone()).collect::<Vec<_>>();
        assert!(disk_recents().contains(&p(0)) && disk_recents().contains(&p(1)));

        // Real Options popover in window A.
        let options_a = all_widgets(ua.window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.tooltip_text().as_deref() == Some("View and folder options"))
            .expect("Options button");
        options_a.emit_clicked();
        let pop = popovers_of(options_a.upcast_ref()).pop().expect("options popover");
        let clear = button_labelled(pop.upcast_ref(), "Clear Recents").expect("Clear Recents row exists");
        assert!(clear.is_sensitive() && clear.is_visible(), "visible and enabled with recents present");
        clear.emit_clicked();
        pump();

        assert!(disk_recents().is_empty(), "disk recents cleared");
        assert_eq!(others(), others_before, "favorites and settings unchanged");
        assert_eq!(tree(), tree_before, "every seeded directory and marker is byte-identical");
        let old_path_shown = |u: &Ui| {
            all_widgets(u.sidebar.upcast_ref()).iter().any(|w| {
                w.downcast_ref::<gtk::Label>().is_some_and(|l| l.label().contains("recent-one-35") || l.label().contains("recent-two-35"))
            })
        };
        assert!(!old_path_shown(&ua) && !old_path_shown(&ub), "no cleared path in either sidebar");
        assert!(sa.borrow().recents.is_empty() && sb.borrow().recents.is_empty(), "both live states empty");
        assert!(!recent_heading(&ua) && !recent_heading(&ub), "no Recent heading in either sidebar");
        let rows_after = all_widgets(ua.sidebar.upcast_ref()).iter().filter(|w| w.is::<gtk::ListBoxRow>()).count();
        assert!(rows_after < rows_before, "the Recent rows are gone");
        assert_eq!(ua.status.label(), "Recent folders cleared");
        assert_eq!(snap(&sa, &ua), snap_a, "window A unchanged");
        assert_eq!(snap(&sb, &ub), snap_b, "window B unchanged");

        // The row stays, disabled.
        options_a.emit_clicked();
        let pop = popovers_of(options_a.upcast_ref()).pop().expect("options popover");
        let clear = button_labelled(pop.upcast_ref(), "Clear Recents").expect("row still present");
        assert!(clear.is_visible() && !clear.is_sensitive(), "visible but disabled when empty");
        pop.popdown();

        // Stale window B navigates: only its new visit appears.
        navigate(&sb, p(5));
        wait_for("new visit", || sb.borrow().cwd == p(5) && sb.borrow().visible_entries.len() == 1);
        assert_eq!(disk_recents(), vec![p(5)], "only the new visit is on disk");
        assert_eq!(sb.borrow().recents.iter().map(|r| r.path.clone()).collect::<Vec<_>>(), vec![p(5)]);
        assert_eq!(others(), others_before);
        assert_eq!(tree(), tree_before, "still byte-identical after the stale-window visit");

        // A fresh window loads no cleared entry.
        update_config_at(&path, |c| c.recents.clear());
        let fresh = read_config_at(&path);
        assert!(fresh.recents.is_empty());
        build_ui(&app, Some(p(4)), None);
        let (sf, _uf) = WINDOWS.with(|w| w.borrow().windows()).into_iter().filter(|w| mine(w)).last().unwrap();
        wait_for("fresh listing", || sf.borrow().visible_entries.len() == 1);
        for old in [p(0), p(1)] {
            assert!(!sf.borrow().recents.iter().any(|r| r.path == old), "old recent resurrected");
            assert!(!disk_recents().contains(&old));
        }

        for (_, u) in WINDOWS.with(|w| w.borrow().windows()).into_iter().filter(|w| mine(w)) {
            u.window.destroy();
        }
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn drive_space_bars_refresh_off_thread_and_stay_cached_d46() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Condvar, Mutex};
        assert_eq!(DRIVE_REFRESH_SECS, 30, "the refresh interval is exactly 30 seconds");
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d46-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let mount_a = root.join("mount-a");
        for d in [&home, &mount_a] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::env::set_var("HOME", &home);
        let gb: u64 = 1024 * 1024 * 1024;
        let drive = |name: &str, mount: Option<&std::path::Path>, space: Option<native_core::Space>| Drive {
            name: name.to_string(),
            path: format!("/dev/synthetic-{name}"),
            mountpoint: mount.map(|m| m.to_string_lossy().to_string()).unwrap_or_default(),
            size: 100 * gb,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable: false,
            space,
        };
        let space = |total_gb: u64, free_gb: u64| Some(native_core::Space { total: total_gb * gb, available: free_gb * gb });
        let listing = Arc::new(Mutex::new(vec![
            drive("SynA", Some(&mount_a), space(100, 25)),
            drive("SynB", None, None),
            drive("SynC", Some(&mount_a), None),
            drive("SynZ", Some(&mount_a), space(0, 0).map(|s| native_core::Space { total: 0, ..s })),
        ]));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let threads = Arc::new(Mutex::new(Vec::new()));
        let main_thread = std::thread::current().id();
        {
            let (listing, gate, calls, threads) = (listing.clone(), gate.clone(), calls.clone(), threads.clone());
            let scan: DriveScan = Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                threads.lock().unwrap().push(std::thread::current().id());
                let (open, cv) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                listing.lock().unwrap().clone()
            });
            TEST_DRIVE_SCAN.with(|s| *s.borrow_mut() = Some(scan));
        }
        #[cfg(feature = "guide")]
        let old_guide = std::env::var("WF_GUIDE_FILE").ok();
        #[cfg(feature = "guide")]
        let guide_file = {
            let gdir = root.join("guide");
            std::fs::create_dir_all(&gdir).unwrap();
            let file = gdir.join("wayfinder.json");
            std::env::set_var("WF_GUIDE_FILE", &file);
            file
        };
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d46test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        // The bar's size comes from the app stylesheet, which build_ui does not install.
        install_app_css();
        build_ui(&app, Some(home.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.present();
        wait_for("first scan started", || calls.load(Ordering::SeqCst) == 1);

        // The scan is blocked: the GTK loop must still run, and repeated requests
        // must not start a second worker.
        let ticks = Rc::new(std::cell::Cell::new(0));
        {
            let ticks = ticks.clone();
            glib::timeout_add_local(Duration::from_millis(10), move || {
                ticks.set(ticks.get() + 1);
                glib::ControlFlow::Continue
            });
        }
        for _ in 0..3 {
            request_drive_refresh(&state);
        }
        pump();
        assert!(ticks.get() >= 3, "GTK loop stalled while the drive scan was blocked");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one refresh in flight at most");
        assert!(state.borrow().drive_refresh_in_flight && state.borrow().drive_refresh_again);
        assert!(state.borrow().drives.is_empty());

        // D-76: the request arrives before any scan result. It waits with no visual and no target.
        #[cfg(feature = "guide")]
        let known = |n: &str, l: &str| GuideRequest::Known { name: n.into(), label: l.into() };
        #[cfg(feature = "guide")]
        let put = |name: &str, label: &str| {
            let tmp = guide_file.with_extension("json.tmp");
            std::fs::write(&tmp, format!(r#"{{"highlight":"{name}","label":"{label}"}}"#)).unwrap();
            std::fs::rename(&tmp, &guide_file).unwrap();
            let want = if name.is_empty() { GuideRequest::Clear } else { known(name, label) };
            wait_for("guide request", || state.borrow().guide_pending == want);
            pump();
        };
        #[cfg(feature = "guide")]
        let tree = || all_widgets(ui.window.upcast_ref());
        #[cfg(feature = "guide")]
        let named = |n: &str| -> Vec<gtk::Widget> { tree().into_iter().filter(|w| w.widget_name() == n).collect() };
        #[cfg(feature = "guide")]
        let label_pops = || -> Vec<gtk::Popover> {
            tree().into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).filter(|p| p.has_css_class("wf-guide-label")).collect()
        };
        #[cfg(feature = "guide")]
        {
            put("drive-space-bars", "Free space for each drive");
            assert!(named("drive-space-bars").is_empty(), "nothing is named before the first scan");
            assert!(guide_marked(tree()).is_empty() && label_pops().is_empty(), "a visual appeared before the scan");
            assert_eq!(state.borrow().guide_pending, known("drive-space-bars", "Free space for each drive"));
        }
        #[cfg(feature = "guide")]
        let one_arrow = |why: &str| -> gtk::Widget {
            wait_for(why, || guide_marked(tree()).len() == 1 && label_pops().len() == 1);
            let marked = guide_marked(tree());
            assert_eq!(marked.len(), 1, "{why}: exactly one target");
            let heading = named("drive-space-bars");
            assert_eq!(heading.len(), 1, "{why}: exactly one named widget");
            assert_eq!(heading[0], marked[0], "{why}: the named heading is the marked one");
            let row = heading[0].downcast_ref::<gtk::ListBoxRow>().expect("the heading row");
            assert!(row.is_mapped() && row.height() > 0);
            let text = row.child().and_downcast::<gtk::Label>().expect("heading label");
            assert_eq!(text.label(), "Devices", "{why}: the heading text is unchanged");
            let pops = label_pops();
            assert!(pops[0].has_css_class("wf-guide-arrow") && !pops[0].can_target() && !pops[0].can_focus());
            let shown = all_widgets(pops[0].upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.label().to_string()));
            assert_eq!(shown.as_deref(), Some("Free space for each drive"), "{why}: exact label");
            let stray: Vec<_> = tree().into_iter().filter(|w| w.widget_name() == "drive-space-bars" && (w.is::<gtk::ProgressBar>() || w.parent().is_some_and(|p| p.is::<gtk::Box>()))).collect();
            assert!(stray.is_empty(), "{why}: no drive row or bar is named");
            marked[0].clone()
        };

        let rows = |ui: &Ui| -> Vec<(String, String, usize, gtk::ListBoxRow)> {
            all_widgets(ui.sidebar.upcast_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
                .map(|row| {
                    let inner = all_widgets(row.upcast_ref());
                    let text = |class: &str| {
                        inner
                            .iter()
                            .filter_map(|w| w.downcast_ref::<gtk::Label>())
                            .find(|l| l.has_css_class(class))
                            .map(|l| l.label().to_string())
                            .unwrap_or_default()
                    };
                    let bars = inner.iter().filter(|w| w.is::<gtk::ProgressBar>()).count();
                    (text("wf-sidebar-title"), text("wf-sidebar-detail"), bars, row.clone())
                })
                .collect()
        };
        let row_named = |ui: &Ui, name: &str| rows(ui).into_iter().find(|r| r.0 == name);
        let geometry = |ui: &Ui| {
            let places = rows(ui).into_iter().find(|r| r.0 == "Home").expect("Home row").3;
            (ui.sidebar.width(), places.height(), places.width(), ui.list_scroll.width(), ui.file_preview.position())
        };
        assert!(row_named(&ui, "SynA").is_none(), "no drive rows before the first scan lands");
        wait_for("laid out", || ui.list_scroll.width() > 0);
        let before = geometry(&ui);

        {
            let (open, cv) = &*gate;
            *open.lock().unwrap() = true;
            cv.notify_all();
        }
        wait_for("rerun after the in-flight scan", || {
            calls.load(Ordering::SeqCst) == 2 && !state.borrow().drive_refresh_in_flight
        });
        assert!(threads.lock().unwrap().iter().all(|t| *t != main_thread), "scan ran on the window thread");
        pump();
        #[cfg(feature = "guide")]
        let first_heading = one_arrow("arrow after the first scan");

        let (_, words, bars, row_a) = row_named(&ui, "SynA").expect("mounted drive row");
        assert_eq!((words.as_str(), bars), ("25 GB available", 1));
        for name in ["SynB", "SynC", "SynZ"] {
            let (_, words, bars, _) = row_named(&ui, name).expect(name);
            assert_eq!((words.as_str(), bars), ("Space unavailable", 0), "{name}");
        }
        let bar = all_widgets(row_a.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ProgressBar>().ok())
            .expect("progress bar");
        assert!((bar.fraction() - 0.75).abs() < 1e-9, "fill is the used share of user-available space");
        assert!(!bar.can_target() && !bar.can_focus());
        assert!(bar.height() > 0 && bar.height() <= 8, "bar is small: {}", bar.height());
        assert_eq!(geometry(&ui), before, "protected sidebar and row geometry moved");

        // Activating a drive row uses the cache: no new scan.
        let scans = calls.load(Ordering::SeqCst);
        activate_sidebar_row(&state, &ui, &row_a);
        wait_for("navigated to the drive", || state.borrow().cwd == mount_a.to_string_lossy());
        assert_eq!(calls.load(Ordering::SeqCst), scans, "row activation scanned drives");

        // A later result replaces the cache: no stale rows, and an unmount clears the bar.
        *listing.lock().unwrap() = vec![drive("SynA", None, None)];
        request_drive_refresh(&state);
        wait_for("replacement scan", || {
            calls.load(Ordering::SeqCst) == scans + 1 && !state.borrow().drive_refresh_in_flight
        });
        pump();
        assert!(["SynB", "SynC", "SynZ"].iter().all(|n| row_named(&ui, n).is_none()), "stale drive rows");
        let (_, words, bars, _) = row_named(&ui, "SynA").expect("unmounted drive row");
        assert_eq!((words.as_str(), bars), ("Space unavailable", 0));

        // D-76: a refresh rebuilds the sidebar. The stale heading loses its arrow before it goes,
        // and one fresh heading is bound after.
        #[cfg(feature = "guide")]
        {
            let refresh = |list: Vec<Drive>| {
                let n = calls.load(Ordering::SeqCst);
                *listing.lock().unwrap() = list;
                request_drive_refresh(&state);
                wait_for("guide refresh scan", || calls.load(Ordering::SeqCst) == n + 1 && !state.borrow().drive_refresh_in_flight);
                pump();
            };
            let stale = first_heading.downgrade();
            drop(first_heading);
            let second = one_arrow("arrow reacquired after the replacement refresh");
            assert!(stale.upgrade().is_none_or(|old| old != second && !old.is_ancestor(&ui.window)), "the stale heading is gone");
            let (_, words, bars, _) = row_named(&ui, "SynA").expect("drive row");
            assert_eq!((words.as_str(), bars), ("Space unavailable", 0), "drive values unchanged by the arrow");
            assert_eq!(geometry(&ui), before, "geometry moved with the arrow drawn");

            let stale = second.downgrade();
            drop(second);
            refresh(vec![drive("SynA", Some(&mount_a), space(100, 25))]);
            let third = one_arrow("arrow after a second refresh");
            assert!(stale.upgrade().is_none_or(|old| old != third));
            let (_, words, bars, _) = row_named(&ui, "SynA").expect("drive row");
            assert_eq!((words.as_str(), bars), ("25 GB available", 1), "the 75% bar is intact under the arrow");
            drop(third);

            // The list empties: no heading, so no visual, but the request stays pending.
            refresh(vec![]);
            assert!(named("drive-space-bars").is_empty());
            assert!(guide_marked(tree()).is_empty() && label_pops().is_empty(), "a visual survived an empty drive list");
            assert_eq!(state.borrow().guide_pending, known("drive-space-bars", "Free space for each drive"));
            assert!(state.borrow().guide_target.as_ref().is_none_or(|t| t.upgrade().is_none()));
            // The drives return: it binds once.
            refresh(vec![drive("SynA", Some(&mount_a), space(100, 25))]);
            let back = one_arrow("arrow reacquired when the drives returned");

            // Another tutorial painted while the sidebar rebuilds is never removed or replaced.
            drop(back);
            put("terminal-here-folder", "Terminal");
            let circle = guide_marked(tree());
            assert_eq!(circle.len(), 1);
            assert_eq!(circle[0].widget_name(), "terminal-here-folder");
            refresh(vec![drive("SynA", None, None)]);
            pump();
            let after = guide_marked(tree());
            assert_eq!(after, circle, "a sidebar rebuild moved another tutorial's visual");
            assert_eq!(state.borrow().guide_pending, known("terminal-here-folder", "Terminal"));

            // Back to this request, then clear: nothing is left.
            put("drive-space-bars", "Free space for each drive");
            one_arrow("arrow after switching back");
            put("", "");
            assert!(guide_marked(tree()).is_empty() && label_pops().is_empty(), "clear left a visual");
            assert!(named("drive-space-bars").len() == 1, "the heading keeps its name; only the visual clears");
            match &old_guide {
                Some(v) => std::env::set_var("WF_GUIDE_FILE", v),
                None => std::env::remove_var("WF_GUIDE_FILE"),
            }
        }

        ui.window.destroy();
        TEST_DRIVE_SCAN.with(|s| *s.borrow_mut() = None);
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn invert_selection_is_the_exact_complement_d29() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d29-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for name in ["alpha.txt", "beta.txt", "gamma.txt", "delta.txt", "apple.png"] {
            std::fs::write(dir.join(name), "data").unwrap();
        }
        std::env::set_var("HOME", &home);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d29test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        wait_for("listing", || state.borrow().visible_entries.len() == 6);

        let names = || {
            let mut v: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        let invariants = || {
            let s = state.borrow();
            (
                names(),
                s.clipboard.is_none(),
                s.undo_stack.len(),
                (s.tabs.tabs[0].history.clone(), s.tabs.tabs[0].index),
                (ui.back.is_sensitive(), ui.forward.is_sensitive()),
                (s.sort_key, s.sort_ascending),
                // the test itself switches views, which the config records; invert must not
                format!("{:?}", AppConfig { view_mode: String::new(), ..load_config() }),
            )
        };
        let before = invariants();
        let options_button = all_widgets(ui.window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.tooltip_text().as_deref() == Some("View and folder options"))
            .expect("Options button");
        let invert = || {
            options_button.emit_clicked();
            let popover = popovers_of(options_button.upcast_ref()).pop().expect("options popover");
            button_labelled(popover.upcast_ref(), "Invert Selection")
                .expect("Invert Selection row")
                .emit_clicked();
            popover.popdown();
            popover.unparent();
        };
        let path = |n: &str| dir.join(n).to_string_lossy().to_string();
        let sel = || {
            let mut v = selected_paths(&state, &ui);
            v.sort();
            v
        };
        let sorted = |names: &[&str]| {
            let mut v: Vec<String> = names.iter().map(|n| path(n)).collect();
            v.sort();
            v
        };
        let all_names = ["alpha.txt", "beta.txt", "gamma.txt", "delta.txt", "apple.png", "sub"];
        let rest_of = |chosen: &[&str]| -> Vec<&str> {
            all_names.iter().copied().filter(|n| !chosen.contains(n)).collect()
        };
        let index_of = |n: &str| state.borrow().visible_entries.iter().position(|e| e.path == path(n)).unwrap();
        let grid = |ui: &Ui| ui.grid.clone();
        let pick = |mode: ViewMode, n: &str, extend: bool| {
            let i = index_of(n);
            if mode == ViewMode::Grid {
                let child = grid(&ui).child_at_index(i as i32).unwrap();
                if !extend {
                    ui.grid.unselect_all();
                }
                ui.grid.select_child(&child);
            } else {
                ui.list_selection.select_item(i as u32, !extend);
            }
        };
        let clear = |mode: ViewMode| {
            if mode == ViewMode::Grid {
                ui.grid.unselect_all()
            } else {
                ui.list_selection.unselect_all();
            }
        };

        for mode in [ViewMode::Details, ViewMode::Compact, ViewMode::Grid] {
            let mode_name = view_mode_id(mode);
            set_view_mode(&state, &ui, mode);
            wait_for("view rows", || {
                if mode == ViewMode::Grid {
                    ui.grid.child_at_index(5).is_some()
                } else {
                    ui.list_selection.n_items() == 6
                }
            });
            let (one, two) = (["beta.txt"], ["alpha.txt", "sub"]);
            pick(mode, "beta.txt", false);
            assert_eq!(sel(), sorted(&one));
            invert();
            assert_eq!(sel(), sorted(&rest_of(&one)), "{mode_name}: complement of one");
            assert!(ui.status.label().contains("5 selected"), "{mode_name}: {}", ui.status.label());
            assert_ne!(ui.preview.label(), "Select a file", "{mode_name}");
            invert();
            assert_eq!(sel(), sorted(&one), "{mode_name}: twice restores");
            assert!(ui.status.label().contains("1 selected"), "{mode_name}: {}", ui.status.label());

            pick(mode, "alpha.txt", false);
            pick(mode, "sub", true);
            assert_eq!(sel(), sorted(&two));
            invert();
            assert_eq!(sel(), sorted(&rest_of(&two)), "{mode_name}: partial");
            invert();
            assert_eq!(sel(), sorted(&two), "{mode_name}: partial twice");

            clear(mode);
            assert!(sel().is_empty());
            invert();
            assert_eq!(sel(), sorted(&all_names), "{mode_name}: none to all");
            assert!(ui.status.label().contains("6 selected"), "{mode_name}: {}", ui.status.label());
            invert();
            assert!(sel().is_empty(), "{mode_name}: all to none");
            assert_eq!(ui.preview.label(), "Select a file", "{mode_name}");
            assert_eq!(view_mode_id(state.borrow().view_mode), mode_name);
            assert_eq!(invariants(), before, "{mode_name}: invariants");
        }
        set_view_mode(&state, &ui, ViewMode::Details);
        wait_for("details rows", || ui.list_selection.n_items() == 6);

        // a filter limits invert to visible rows; no filtered-out path is selected
        ui.search.set_text("a.txt");
        wait_for("filtered", || state.borrow().visible_entries.len() == 4);
        let visible: Vec<String> = state.borrow().visible_entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(visible.len(), 4, "{visible:?}");
        assert!(visible.iter().all(|p| p.ends_with("a.txt")), "{visible:?}");
        pick(ViewMode::Details, "alpha.txt", false);
        pick(ViewMode::Details, "beta.txt", true);
        invert();
        assert_eq!(sel(), sorted(&["gamma.txt", "delta.txt"]), "filtered complement");
        invert();
        assert_eq!(sel(), sorted(&["alpha.txt", "beta.txt"]), "filtered twice");
        assert!(sel().iter().all(|p| visible.contains(p)));

        // zero visible rows: safe, nothing selected
        ui.search.set_text("zzz-no-match");
        wait_for("no rows", || state.borrow().visible_entries.is_empty());
        invert();
        assert!(sel().is_empty());
        assert!(ui.list_selection.selection().is_empty());
        ui.search.set_text("");
        wait_for("unfiltered", || state.borrow().visible_entries.len() == 6);
        assert!(sel().is_empty());
        assert_eq!(invariants(), before, "final invariants");
        ui.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn show_hidden_toolbar_and_options_stay_in_sync_d28() {
        let old_home = std::env::var("HOME").ok();
        let root = crate::native_core::scratch_root().join(format!("wf-d28-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(dir.join(".hiddir")).unwrap();
        std::fs::write(dir.join("vis.txt"), "v").unwrap();
        std::fs::write(dir.join(".hid.txt"), "h").unwrap();
        std::env::set_var("HOME", &home);
        let dir_str = dir.to_string_lossy().to_string();
        let open = |id: &str| {
            let app = adw::Application::builder()
                .application_id(id)
                .flags(gio::ApplicationFlags::NON_UNIQUE)
                .build();
            app.register(gio::Cancellable::NONE).expect("register test application");
            build_ui(&app, Some(dir_str.clone()), None);
            let found = WINDOWS
                .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
                .expect("the test window is registered");
            (app, found.0, found.1)
        };
        let saved = || load_config().show_hidden;
        let option_row = |ui: &Ui| {
            let options = all_widgets(ui.window.upcast_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.tooltip_text().as_deref() == Some("View and folder options"))
                .expect("Options button");
            options.emit_clicked();
            let popover = popovers_of(options.upcast_ref()).pop().expect("options popover");
            let row = all_widgets(popover.upcast_ref())
                .into_iter()
                .find(|w| w.widget_name() == "wf-show-hidden-option")
                .and_then(|w| w.downcast::<gtk::Button>().ok())
                .expect("Show Hidden Files row");
            (popover, row)
        };
        // The row is checked exactly when its icon is the check mark and it has the class.
        let row_checked = |row: &gtk::Button| {
            let icon = all_widgets(row.upcast_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Image>().ok())
                .any(|i| i.icon_name().as_deref() == Some("object-select-symbolic"));
            assert_eq!(icon, row.has_css_class("wf-checked"));
            icon
        };
        let close = |popover: gtk::Popover| {
            popover.popdown();
            popover.unparent();
        };

        let (_app, state, ui) = open("com.kadaken.wayfinder.d28test");
        wait_for("listing", || state.borrow().visible_entries.len() == 1);
        let tab = || {
            let s = state.borrow();
            (s.tabs.tabs[0].history.clone(), s.tabs.tabs[0].index)
        };
        let nav = || (ui.back.is_sensitive(), ui.forward.is_sensitive());
        let (tab0, nav0) = (tab(), nav());

        // reachable: a descendant of the visible window and of the command bar
        assert!(ui.hidden.is_ancestor(&ui.window) && ui.hidden.is_mapped());
        let mut in_bar = false;
        let mut w = ui.hidden.parent();
        while let Some(p) = w {
            in_bar |= p.has_css_class("wf-commandbar");
            w = p.parent();
        }
        assert!(in_bar, "the toggle sits in the command bar");

        // saved false: both surfaces off, hidden rows absent
        assert!(!saved() && !state.borrow().show_hidden && !ui.hidden.is_active());
        let (pop, row) = option_row(&ui);
        assert!(!row_checked(&row));
        close(pop);
        assert!(state.borrow().visible_entries.iter().all(|e| !e.name.starts_with('.')));

        // real toolbar signal: on
        ui.hidden.set_active(true);
        wait_for("hidden rows shown", || state.borrow().visible_entries.len() == 3);
        assert!(saved() && state.borrow().show_hidden);
        let (pop, row) = option_row(&ui);
        assert!(row_checked(&row), "Options follows the toolbar");
        assert_eq!((tab(), nav()), (tab0.clone(), nav0));

        // real Options control: off
        row.emit_clicked();
        wait_for("hidden rows removed", || state.borrow().visible_entries.len() == 1);
        assert!(!ui.hidden.is_active() && !saved() && !state.borrow().show_hidden);
        close(pop);
        let (pop, row) = option_row(&ui);
        assert!(!row_checked(&row));
        close(pop);
        assert_eq!((tab(), nav()), (tab0.clone(), nav0));

        // Options control: on, then re-selecting the current value is a no-op
        let (pop, row) = option_row(&ui);
        row.emit_clicked();
        wait_for("shown again", || state.borrow().visible_entries.len() == 3);
        assert!(ui.hidden.is_active() && saved());
        close(pop);
        let generation = state.borrow().listing_generation;
        set_show_hidden(&state, true);
        assert_eq!(state.borrow().listing_generation, generation, "no second refresh");
        assert_eq!((tab(), nav()), (tab0, nav0));

        // a new window starts with both controls on and the rows visible
        ui.window.destroy();
        let (_app2, state2, ui2) = open("com.kadaken.wayfinder.d28test2");
        wait_for("new window listing", || state2.borrow().visible_entries.len() == 3);
        assert!(ui2.hidden.is_active() && state2.borrow().show_hidden);
        let (pop, row) = option_row(&ui2);
        assert!(row_checked(&row));
        close(pop);
        ui2.window.destroy();
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn tabs_keep_independent_history_d27() {
        let root = crate::native_core::scratch_root().join(format!("wf-d27-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mk = |n: &str| {
            let d = root.join(n);
            std::fs::create_dir_all(&d).unwrap();
            d.to_string_lossy().to_string()
        };
        let (a0, a1, a2, b0, b1) = (mk("a0"), mk("a1"), mk("a2"), mk("b0"), mk("b1"));
        let missing = root.join("missing").to_string_lossy().to_string();
        std::env::set_var("HOME", &home);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d27test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(a0.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        let at = |p: &str| {
            let (state, p) = (state.clone(), p.to_string());
            move || state.borrow().cwd == p
        };
        let hist = |i: usize| {
            let s = state.borrow();
            (s.tabs.tabs[i].history.clone(), s.tabs.tabs[i].index)
        };
        let v = |x: &[&String]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        wait_for("a0", at(&a0));
        for p in [&a1, &a2] {
            navigate(&state, p.clone());
            wait_for("nav", at(p));
        }
        assert_eq!(hist(0), (v(&[&a0, &a1, &a2]), 2));

        // a new tab and Open in New Tab each start with exactly one entry
        assert!(ui.back.is_sensitive(), "A at its end has Back");
        open_path_in_new_tab(&state, &ui, b0.clone());
        assert!(!ui.back.is_sensitive() && !ui.forward.is_sensitive(), "one-entry tab, before the listing");
        wait_for("b0", at(&b0));
        assert_eq!(hist(1), (v(&[&b0]), 0));
        navigate(&state, b1.clone());
        wait_for("b1", at(&b1));
        assert_eq!(hist(1), (v(&[&b0, &b1]), 1));
        assert_eq!(hist(0), (v(&[&a0, &a1, &a2]), 2), "A untouched by B");
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive());

        // Back/Forward in B stay inside B
        history_step(&state, false);
        wait_for("b back", at(&b0));
        assert_eq!(hist(1), (v(&[&b0, &b1]), 0));
        assert!(!ui.back.is_sensitive() && ui.forward.is_sensitive());
        history_step(&state, true);
        wait_for("b fwd", at(&b1));

        // repeated switching grows nothing and restores each tab
        for _ in 0..3 {
            switch_tab_action(state.clone(), 0);
            assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "A's state before its listing");
            wait_for("to A", at(&a2));
            assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive());
            switch_tab_action(state.clone(), 1);
            assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "B's state before its listing");
            wait_for("to B", at(&b1));
        }
        history_step(&state, false);
        wait_for("b0 for sensitivity", at(&b0));
        switch_tab_action(state.clone(), 0);
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "A after B was at its start");
        wait_for("A", at(&a2));
        switch_tab_action(state.clone(), 1);
        assert!(!ui.back.is_sensitive() && ui.forward.is_sensitive(), "B at its start, before the listing");
        wait_for("B", at(&b0));
        history_step(&state, true);
        wait_for("b1 restored", at(&b1));
        assert_eq!(hist(0), (v(&[&a0, &a1, &a2]), 2));
        assert_eq!(hist(1), (v(&[&b0, &b1]), 1));

        // A: Back to a1/a0, then a new navigation truncates only A's forward branch
        switch_tab_action(state.clone(), 0);
        wait_for("A", at(&a2));
        history_step(&state, false);
        wait_for("a1", at(&a1));
        history_step(&state, false);
        wait_for("a0", at(&a0));
        assert_eq!(hist(0), (v(&[&a0, &a1, &a2]), 0));
        history_step(&state, true);
        wait_for("a1 again", at(&a1));
        navigate(&state, b0.clone());
        wait_for("branch", at(&b0));
        assert_eq!(hist(0), (v(&[&a0, &a1, &b0]), 2));
        assert_eq!(hist(1), (v(&[&b0, &b1]), 1), "B unchanged by A's branch");

        // a failed listing and a superseded one write to no tab
        let before = (hist(0), hist(1));
        navigate(&state, missing.clone());
        wait_for("failure reported", || ui.status.label().contains("Cannot open"));
        assert_eq!((hist(0), hist(1)), before);
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "A's own state after a failed listing");
        navigate(&state, a2.clone());
        navigate(&state, a1.clone());
        wait_for("superseding load", at(&a1));
        pump();
        assert_eq!(hist(0), (v(&[&a0, &a1, &b0, &a1]), 3), "only the newest load recorded");
        // a load pending while the tab changes lands nowhere
        let before = (hist(0), hist(1));
        navigate(&state, a2.clone());
        switch_tab_action(state.clone(), 1);
        wait_for("B shown", at(&b1));
        pump();
        assert_eq!((hist(0), hist(1)), before);
        assert_eq!(state.borrow().cwd, b1);

        // a failed listing leaves the ACTIVE tab's sensitivity, not the prior tab's
        history_step(&state, false);
        wait_for("B at start", at(&b0));
        switch_tab_action(state.clone(), 0);
        wait_for("A shown", at(&a1));
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive());
        navigate(&state, missing.clone());
        wait_for("second failure", || ui.status.label().contains("Cannot open"));
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "A's state, not B's");
        switch_tab_action(state.clone(), 1);
        wait_for("B again", at(&b0));
        history_step(&state, true);
        wait_for("B restored", at(&b1));
        assert_eq!((hist(0), hist(1)), before);

        // closing B keeps A's exact history and cursor
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive());
        close_tab_action(state.clone(), ui.clone());
        assert!(ui.back.is_sensitive() && !ui.forward.is_sensitive(), "A's Back restored before the listing");
        wait_for("A after close", at(&a1));
        assert_eq!(state.borrow().tabs.tabs.len(), 1);
        assert_eq!(hist(0), before.0);
    }

    fn metadata_refresh_keeps_row_identity_and_structure_rebuilds_d22() {
        let root = crate::native_core::scratch_root().join(format!("wf-d22-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "1").unwrap();
        }
        std::env::set_var("HOME", &home);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d22test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        let dir_str = dir.to_string_lossy().to_string();
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        wait_for("listing", || state.borrow().visible_entries.len() == 3);
        let b_path = dir.join("b.txt").to_string_lossy().to_string();
        let b_labels = |state: &Rc<RefCell<AppState>>| {
            state
                .borrow()
                .meta_labels
                .get(&b_path)
                .and_then(|(size, modified)| Some((size.upgrade()?, modified.upgrade()?)))
        };

        // ---- Details: a real monitor event updates size/time in place.
        wait_for("row b realized", || b_labels(&state).is_some());
        select_entry_by_path(&state, &ui, &b_path);
        let (size_before, modified_before) = b_labels(&state).unwrap();
        let row_before = size_before.parent().unwrap();
        let object_before = ui.list_store.item(1).unwrap();
        assert_eq!(size_before.label(), "1 B");
        std::fs::write(dir.join("b.txt"), vec![b'x'; 2048]).unwrap();
        wait_for("size text changes", || size_before.label() != "1 B");
        let (size_after, modified_after) = b_labels(&state).unwrap();
        assert_eq!(size_after, size_before, "the same size label object");
        assert_eq!(modified_after, modified_before, "the same modified label object");
        assert_eq!(size_after.parent().unwrap(), row_before, "the same row widget");
        assert_eq!(ui.list_store.item(1).unwrap(), object_before, "the same model object, no splice");
        assert_eq!(size_after.label(), native_core::fmt_size(2048));
        assert_eq!(selected_paths(&state, &ui), vec![b_path.clone()], "selection kept");
        assert_eq!(state.borrow().visible_entries[1].size, 2048);
        assert_eq!(state.borrow().entries.iter().find(|e| e.path == b_path).unwrap().size, 2048);
        assert_eq!(object_before.downcast::<glib::BoxedAnyObject>().unwrap().borrow::<Entry>().size, 2048);

        // ---- Structural: a created file rebuilds, the selection survives.
        std::fs::write(dir.join("d.txt"), "d").unwrap();
        wait_for("structural rebuild", || state.borrow().visible_entries.len() == 4);
        wait_for("selection restored", || selected_paths(&state, &ui) == vec![b_path.clone()]);
        // ---- Structural: a deletion rebuilds too.
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        wait_for("deletion rebuild", || state.borrow().visible_entries.len() == 3);
        wait_for("selection restored again", || selected_paths(&state, &ui) == vec![b_path.clone()]);

        // ---- Compact: a second metadata-only change keeps every object.
        set_view_mode(&state, &ui, ViewMode::Compact);
        wait_for("compact rows", || b_labels(&state).is_some());
        select_entry_by_path(&state, &ui, &b_path);
        let (c_size, c_modified) = b_labels(&state).unwrap();
        let c_row = c_size.parent().unwrap();
        let b_index = state.borrow().visible_entries.iter().position(|e| e.path == b_path).unwrap() as u32;
        let c_object = ui.list_store.item(b_index).unwrap();
        let c_text = c_size.label();
        std::fs::write(dir.join("b.txt"), vec![b'z'; 8192]).unwrap();
        wait_for("compact size text advances", || c_size.label() != c_text);
        let (c_size_after, c_modified_after) = b_labels(&state).unwrap();
        assert_eq!(c_size_after, c_size, "Compact: same size label object");
        assert_eq!(c_modified_after, c_modified, "Compact: same modified label object");
        assert_eq!(c_size_after.parent().unwrap(), c_row, "Compact: same row widget");
        assert_eq!(ui.list_store.item(b_index).unwrap(), c_object, "Compact: same model object");
        assert_eq!(c_size_after.label(), native_core::fmt_size(8192));
        assert_eq!(selected_paths(&state, &ui), vec![b_path.clone()], "Compact: selection kept");

        // ---- Grid: a metadata-only scan does not remove or recreate children.
        set_view_mode(&state, &ui, ViewMode::Grid);
        wait_for("grid children", || ui.grid.observe_children().n_items() == 3);
        select_entry_by_path(&state, &ui, &b_path);
        let children_before: Vec<gtk::Widget> = (0..3)
            .map(|i| ui.grid.child_at_index(i).unwrap().upcast())
            .collect();
        std::fs::write(dir.join("b.txt"), vec![b'y'; 4096]).unwrap();
        wait_for("grid state merged", || {
            state.borrow().visible_entries.iter().any(|e| e.path == b_path && e.size == 4096)
        });
        for _ in 0..40 {
            pump();
        }
        let children_after: Vec<gtk::Widget> = (0..3)
            .map(|i| ui.grid.child_at_index(i).unwrap().upcast())
            .collect();
        assert_eq!(children_before, children_after, "same Grid child objects");
        assert_eq!(selected_paths(&state, &ui), vec![b_path.clone()]);
        // Grid structural change still rebuilds and keeps the selection.
        std::fs::write(dir.join("e.txt"), "e").unwrap();
        wait_for("grid rebuild", || ui.grid.observe_children().n_items() == 4);
        wait_for("grid selection restored", || selected_paths(&state, &ui) == vec![b_path.clone()]);

        ui.window.destroy();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Joins a background writer on success and on panic.
    struct WriterGuard {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl WriterGuard {
        /// Appends to `path` every 40 ms until stopped; bounded to 30 s in any case.
        fn start(path: std::path::PathBuf) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let handle = std::thread::spawn(move || {
                use std::io::Write;
                let began = Instant::now();
                let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
                while !flag.load(Ordering::SeqCst) && began.elapsed() < Duration::from_secs(30) {
                    let _ = file.write_all(&[b'g'; 700]);
                    let _ = file.flush();
                    std::thread::sleep(Duration::from_millis(40));
                }
            });
            Self { stop, handle: Some(handle) }
        }

        fn finish(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    impl Drop for WriterGuard {
        fn drop(&mut self) {
            self.finish();
        }
    }

    /// A background writer grows `g.part` for 2.6+ seconds in each view. Selection
    /// of a stable row is driven in code; the same row object and selection must
    /// survive every refresh, and scan starts are timed. A real pointer click and
    /// hover are not covered here: they are carried to the watched gate.
    fn sustained_stream_keeps_objects_pointer_and_timing_d22() {
        let root = crate::native_core::scratch_root().join(format!("wf-d22s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "g.part"] {
            std::fs::write(dir.join(name), "1").unwrap();
        }
        std::env::set_var("HOME", &home);
        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d22stream")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        build_ui(&app, Some(dir.to_string_lossy().to_string()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        ui.window.set_default_size(900, 600);
        ui.window.present();
        wait_for("listing", || state.borrow().visible_entries.len() == 4);
        let a_path = dir.join("a.txt").to_string_lossy().to_string();
        let b_path = dir.join("b.txt").to_string_lossy().to_string();
        let index_of = |path: &str| state.borrow().visible_entries.iter().position(|e| e.path == path).unwrap();
        let row_widget = |mode: ViewMode, path: &str| -> Option<gtk::Widget> {
            if mode == ViewMode::Grid {
                ui.grid.child_at_index(index_of(path) as i32).map(|c| c.upcast())
            } else {
                state
                    .borrow()
                    .meta_labels
                    .get(path)
                    .and_then(|(size, _)| size.upgrade())
                    .and_then(|l| l.parent())
            }
        };
        let mut report = Vec::new();

        for mode in [ViewMode::Details, ViewMode::Compact, ViewMode::Grid] {
            let mode_name = match mode {
                ViewMode::Details => "Details",
                ViewMode::Compact => "Compact",
                ViewMode::Grid => "Grid",
            };
            set_view_mode(&state, &ui, mode);
            wait_for("rows realized", || {
                row_widget(mode, &b_path).is_some_and(|r| r.width() > 0 && r.height() > 0)
            });
            for _ in 0..5 {
                pump();
            }
            select_entry_by_path(&state, &ui, &a_path);
            assert_eq!(selected_paths(&state, &ui), vec![a_path.clone()]);

            // Earlier fixture events must drain before the timed stream: no scan
            // may start for 1.3 s.
            let mut quiet_since = monotonic_ms();
            let mut seen = SCAN_LOG.with(|log| log.borrow().len());
            while monotonic_ms() - quiet_since < 1300 {
                pump();
                let now_seen = SCAN_LOG.with(|log| log.borrow().len());
                if now_seen != seen {
                    seen = now_seen;
                    quiet_since = monotonic_ms();
                }
            }
            SCAN_LOG.with(|log| log.borrow_mut().clear());
            let t0 = monotonic_ms();
            let mut writer = WriterGuard::start(dir.join("g.part"));

            // The stable row is selected in code while the writer runs; the real
            // pointer click is carried to the watched gate.
            let row = row_widget(mode, &b_path).expect("b.txt row is realized");
            select_entry_by_path(&state, &ui, &b_path);
            assert_eq!(selected_paths(&state, &ui), vec![b_path.clone()]);
            let object = ui.list_store.item(index_of(&b_path) as u32);

            // Refreshes keep running while the writer grows the file.
            while monotonic_ms() - t0 < 2800 {
                pump();
                assert_eq!(selected_paths(&state, &ui), vec![b_path.clone()], "{mode_name}: selection kept");
                assert_eq!(row_widget(mode, &b_path).expect("row still realized"), row, "{mode_name}: same row object");
                if mode != ViewMode::Grid {
                    assert_eq!(ui.list_store.item(index_of(&b_path) as u32), object, "{mode_name}: same model object");
                }
            }
            writer.finish();
            let grown = std::fs::metadata(dir.join("g.part")).unwrap().len();
            let log = SCAN_LOG.with(|log| log.borrow().clone());
            assert!(log.len() >= 3, "{mode_name}: scans keep running: {log:?}");
            assert!(log[0].0 >= t0 + 500, "{mode_name}: the first scan waits 500 ms: t0={t0} {log:?}");
            for pair in log.windows(2) {
                assert!(pair[1].0 >= pair[0].0 + 500, "{mode_name}: scans start 500 ms apart: {log:?}");
                assert!(pair[0].1 != 0 && pair[1].0 >= pair[0].1, "{mode_name}: scans never overlap: {log:?}");
            }
            assert!(grown > 5000, "{mode_name}: the background writer really grew the file: {grown}");
            report.push(format!(
                "{mode_name}: selected {} ; scans {} ; g.part {grown} bytes",
                b_path.rsplit('/').next().unwrap(),
                log.len()
            ));
        }
        eprintln!("wf-event: d22 sustained writer gate: {report:?}");
        ui.window.destroy();
    }

    /// Removes a scratch fixture on success and on a failed assertion.
    struct ScratchCleanup(std::path::PathBuf);
    impl Drop for ScratchCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn committed_transfer_warning_keeps_undo_and_an_error_has_none_d24() {
        let item = native_core::ResolvedItem { from: "/s/a".into(), to: "/d/a".into() };
        let committed = native_core::TransferResult {
            items: vec![item.clone()],
            warnings: vec!["could not remove a backup".into()],
        };
        let outcome = transfer_outcome("Pasted", true, Ok(committed)).unwrap();
        assert!(outcome.message.contains("warning"));
        assert!(matches!(outcome.undo, Some(UndoAction::Pasted { was_copy: true, ref items }) if items.len() == 1));
        assert!(transfer_outcome("Pasted", true, Err("rolled back".into())).is_err());
    }

    #[test]
    fn tab_history_model_is_per_tab_d27() {
        let mut set = TabSet::new("/a0".into());
        set.active_mut().record("/a1");
        set.active_mut().record("/a1");
        assert_eq!(set.active().history, ["/a0", "/a1"], "same path is not appended twice");
        set.open("/b0".into());
        assert_eq!(set.active().history, ["/b0"]);
        assert_eq!(set.active().id, 1);
        assert!(set.back_target().is_none() && set.forward_target().is_none());
        assert_eq!(set.switch(0).as_deref(), Some("/a1"));
        assert_eq!(set.back_target(), Some((0, "/a0".into())));
        assert_eq!(set.switch(9), None);
        set.open("/c0".into());
        assert_eq!(set.active().id, 2);
        set.switch(0);
        assert_eq!(set.close_active().as_deref(), Some("/b0"), "closing keeps the next tab's history");
        assert_eq!(set.active().history, ["/b0"]);
        assert_eq!(set.close_active().as_deref(), Some("/c0"));
        assert_eq!(set.close_active(), None, "the last tab stays");
    }

    #[test]
    fn only_a_verified_mountpoint_navigates_d26() {
        let ok = mount_outcome(Ok(native_core::Mounted::New("/run/media/u/USB DRIVE".into()))).unwrap();
        assert_eq!(ok.message, "Mounted at /run/media/u/USB DRIVE");
        assert_eq!(ok.refresh_to.as_deref(), Some("/run/media/u/USB DRIVE"));
        assert!(ok.undo.is_none());
        let already = mount_outcome(Ok(native_core::Mounted::Already("/run/media/u/USB DRIVE".into()))).unwrap();
        assert_eq!(already.message, "Already mounted at /run/media/u/USB DRIVE");
        assert_eq!(already.refresh_to.as_deref(), Some("/run/media/u/USB DRIVE"));
        assert!(already.undo.is_none());
        let err = mount_outcome(Err("Mount command completed but no usable mountpoint was found".into()));
        assert!(err.is_err());
    }

    fn descendants(root: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        let mut child = root.first_child();
        while let Some(c) = child {
            out.push(c.clone());
            descendants(&c, out);
            child = c.next_sibling();
        }
    }

    fn all_widgets(root: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut out = vec![root.clone()];
        descendants(root, &mut out);
        out
    }

    /// The button in `root`'s tree whose visible label is exactly `label`.
    fn button_labelled(root: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        all_widgets(root).into_iter().find_map(|w| {
            let button = w.downcast_ref::<gtk::Button>()?;
            let has = all_widgets(button.upcast_ref())
                .iter()
                .any(|c| c.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == label));
            has.then(|| button.clone())
        })
    }

    fn popovers_of(widget: &gtk::Widget) -> Vec<gtk::Popover> {
        all_widgets(widget).into_iter().filter_map(|w| w.downcast::<gtk::Popover>().ok()).collect()
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..80 {
            if cond() {
                return;
            }
            pump();
        }
        panic!("timed out waiting for {what}");
    }

    fn inline_field(root: &gtk::Widget) -> Option<gtk::Entry> {
        all_widgets(root)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Entry>().ok())
            .find(|e| e.has_css_class("wf-file-name"))
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    /// D-21: New > Folder from the file-pane background (Details and Grid), the
    /// toolbar, the options menu and Ctrl+Shift+N all reach one action, which
    /// creates a real numbered folder and opens the shared inline editor on it.
    fn new_folder_menu_and_shared_inline_editor_d21() {
        let root = crate::native_core::scratch_root().join(format!("wf-d21-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = ScratchCleanup(root.clone());
        let home = root.join("home");
        let dir = root.join("dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(dir.join("New Folder")).unwrap();
        std::fs::write(dir.join("aaa.txt"), "a").unwrap();
        std::fs::write(dir.join("taken"), "t").unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);

        let app = adw::Application::builder()
            .application_id("com.kadaken.wayfinder.d21test")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).expect("register test application");
        let dir_str = dir.to_string_lossy().to_string();
        build_ui(&app, Some(dir_str.clone()), None);
        let (state, ui) = WINDOWS
            .with(|w| w.borrow().active(|ui| ui.window.application().as_ref() == Some(app.upcast_ref())))
            .expect("the test window is registered");
        wait_for("listing", || state.borrow().visible_entries.len() == 3);
        let win: gtk::Widget = ui.window.clone().upcast();
        let created = |name: &str| dir.join(name);

        // ---- Details: background menu, real traversal New -> Folder.
        let undo_len = |s: &Rc<RefCell<AppState>>| s.borrow().undo_stack.len();
        let undo_before = undo_len(&state);
        show_background_context_menu(&ui.list, state.clone(), ui.clone(), 10.0, 10.0);
        let menu_popover = popovers_of(ui.list.upcast_ref()).pop().expect("background popover");
        assert!(button_labelled(menu_popover.upcast_ref(), "New Folder").is_none(), "no top-level New Folder row");
        assert!(button_labelled(menu_popover.upcast_ref(), "Folder").is_none(), "Folder is inside New, not top level");
        let new_row = button_labelled(menu_popover.upcast_ref(), "New").expect("a New row");
        new_row.emit_clicked();
        let side = popovers_of(new_row.upcast_ref()).pop().expect("New opens a submenu");
        assert!(button_labelled(side.upcast_ref(), "File").is_some(), "New File lives under New");
        button_labelled(side.upcast_ref(), "Folder").expect("Folder under New").emit_clicked();
        wait_for("the folder on disk", || created("New Folder (2)").is_dir());
        wait_for("the inline editor", || inline_field(ui.list.upcast_ref()).is_some());
        let field = inline_field(ui.list.upcast_ref()).unwrap();
        assert_eq!(field.text(), "New Folder (2)");
        let full = created("New Folder (2)").to_string_lossy().to_string();
        assert_eq!(state.borrow().inline_rename.as_ref().map(|e| e.path.clone()), Some(full.clone()));
        assert_eq!(selected_paths(&state, &ui), vec![full.clone()], "the new folder is selected");
        wait_for("full selection and focus", || {
            field.selection_bounds() == Some((0, 14)) && focus_within(&ui.window, &field)
        });
        assert_eq!(undo_len(&state), undo_before + 1);
        assert!(matches!(state.borrow().undo_stack.last(), Some(UndoAction::Created { paths }) if paths == &vec![full.clone()]));

        // Enter renames exactly once; a second Enter writes nothing more.
        field.set_text("Renamed");
        field.emit_activate();
        field.emit_activate();
        wait_for("rename", || created("Renamed").is_dir() && !created("New Folder (2)").exists());
        assert!(matches!(state.borrow().undo_stack.last(), Some(UndoAction::Renamed { .. })));
        assert_eq!(undo_len(&state), undo_before + 2, "one Created and one Renamed, no double write");
        wait_for("editor gone", || inline_field(ui.list.upcast_ref()).is_none());

        // ---- Toolbar: same action; Escape keeps the generated folder.
        let toolbar_button = |tip: &str| {
            all_widgets(&win)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.tooltip_text().as_deref() == Some(tip))
                .unwrap_or_else(|| panic!("toolbar button {tip}"))
        };
        toolbar_button("New folder").emit_clicked();
        wait_for("toolbar folder", || created("New Folder (2)").is_dir());
        wait_for("toolbar editor", || inline_field(ui.list.upcast_ref()).is_some());
        cancel_inline_rename(&state, &ui);
        wait_for("editor closed", || inline_field(ui.list.upcast_ref()).is_none());
        assert!(created("New Folder (2)").is_dir(), "cancelling leaves the created folder");
        assert!(state.borrow().inline_rename.is_none());

        // ---- Options menu: same action.
        let options = toolbar_button("View and folder options");
        options.emit_clicked();
        let options_popover = popovers_of(options.upcast_ref()).pop().expect("options popover");
        button_labelled(options_popover.upcast_ref(), "New Folder").expect("options New Folder").emit_clicked();
        wait_for("options folder", || created("New Folder (3)").is_dir());
        wait_for("options editor", || inline_field(ui.list.upcast_ref()).is_some());
        cancel_inline_rename(&state, &ui);

        // ---- Ctrl+Shift+N through the window's real key controller.
        let handled = ui
            .window
            .observe_controllers()
            .into_iter()
            .filter_map(|c| c.ok()?.downcast::<gtk::EventControllerKey>().ok())
            .any(|key| {
                key.emit_by_name::<bool>(
                    "key-pressed",
                    &[&gdk::Key::N, &0u32, &(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)],
                )
            });
        assert!(handled, "Ctrl+Shift+N is handled");
        wait_for("shortcut folder", || created("New Folder (4)").is_dir());
        wait_for("shortcut editor", || inline_field(ui.list.upcast_ref()).is_some());

        // A colliding rename stays open with its exact reason and changes nothing.
        let field = inline_field(ui.list.upcast_ref()).unwrap();
        field.set_text("taken");
        field.emit_activate();
        wait_for("collision error", || state.borrow().inline_rename.as_ref().is_some_and(|e| e.error.is_some()));
        assert!(created("New Folder (4)").is_dir() && created("taken").is_file());
        assert!(inline_field(ui.list.upcast_ref()).is_some(), "the editor stays open");
        cancel_inline_rename(&state, &ui);

        // ---- Grid: background menu on the grid surface, same editor and commit.
        set_view_mode(&state, &ui, ViewMode::Grid);
        wait_for("grid children", || ui.grid.child_at_index(0).is_some());
        let grid_scroll = ui.files_stack.child_by_name("grid").expect("grid scroller");
        let secondary = all_widgets(&grid_scroll)[..1]
            .iter()
            .flat_map(|w| {
                w.observe_controllers()
                    .into_iter()
                    .filter_map(|c| c.ok()?.downcast::<gtk::GestureClick>().ok())
                    .filter(|g| g.button() == 3)
                    .collect::<Vec<_>>()
            })
            .count();
        assert_eq!(secondary, 1, "the grid surface has a right-click handler");
        show_background_context_menu(&grid_scroll, state.clone(), ui.clone(), 10.0, 10.0);
        let grid_menu = popovers_of(&grid_scroll).pop().expect("grid background popover");
        let new_row = button_labelled(grid_menu.upcast_ref(), "New").expect("grid New row");
        new_row.emit_clicked();
        let side = popovers_of(new_row.upcast_ref()).pop().expect("grid submenu");
        button_labelled(side.upcast_ref(), "Folder").expect("grid Folder").emit_clicked();
        wait_for("grid folder", || created("New Folder (4)").is_dir() && dir_names(&dir).iter().any(|n| n == "New Folder (5)"));
        wait_for("grid editor", || inline_field(ui.grid.upcast_ref()).is_some());
        let field = inline_field(ui.grid.upcast_ref()).unwrap();
        assert_eq!(field.text(), "New Folder (5)");
        wait_for("grid selection", || {
            field.selection_bounds() == Some((0, 14)) && focus_within(&ui.window, &field)
        });
        assert_eq!(selected_paths(&state, &ui), vec![created("New Folder (5)").to_string_lossy().to_string()]);
        field.set_text("GridNamed");
        field.emit_activate();
        field.emit_activate();
        wait_for("grid rename", || created("GridNamed").is_dir() && !created("New Folder (5)").exists());
        wait_for("grid editor gone", || inline_field(ui.grid.upcast_ref()).is_none());
        // Cancel in Grid keeps the folder too.
        create_folder_action(state.clone(), ui.clone());
        wait_for("grid folder 2", || created("New Folder (5)").is_dir());
        wait_for("grid editor 2", || inline_field(ui.grid.upcast_ref()).is_some());
        cancel_inline_rename(&state, &ui);
        wait_for("grid editor closed", || inline_field(ui.grid.upcast_ref()).is_none());
        assert!(created("New Folder (5)").is_dir());
        set_view_mode(&state, &ui, ViewMode::Details);

        // ---- Failure leaves no folder, editor or undo entry.
        use std::os::unix::fs::PermissionsExt;
        let before_names = dir_names(&dir);
        let undo_before = undo_len(&state);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        create_folder_action(state.clone(), ui.clone());
        wait_for("failure status", || ui.status.label().starts_with("Operation failed"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        pump();
        assert_eq!(dir_names(&dir), before_names, "disk unchanged");
        assert!(state.borrow().inline_rename.is_none() && state.borrow().reveal_path.is_none());
        assert_eq!(undo_len(&state), undo_before, "no undo entry for a failed creation");
        assert!(inline_field(ui.list.upcast_ref()).is_none());

        ui.window.close();
        pump();
        match old_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        std::fs::remove_dir_all(&root).ok();
        drop(win);
    }

    /// D-20: removing the last row of a folder while focus sat on a file row
    /// made GTK's next-frame focus search hit the selectable preview label and
    /// raise a critical (fatal under test). Focus must move to the surviving
    /// view root, and only when it was in the file pane.
    fn emptied_folder_keeps_focus_on_the_view_root_d20() {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let text = item.item().and_downcast::<gtk::StringObject>().unwrap().string();
            item.set_child(Some(&gtk::Label::new(Some(&text))));
        });
        let store = gio::ListStore::new::<gtk::StringObject>();
        store.append(&gtk::StringObject::new("only-row"));
        let list = gtk::ListView::new(Some(gtk::MultiSelection::new(Some(store.clone()))), Some(factory));
        let grid = gtk::FlowBox::new();
        let grid_label = gtk::Label::new(Some("grid-item"));
        grid.append(&grid_label);
        let entry = gtk::Entry::new();
        let preview = gtk::Label::new(Some("preview"));
        preview.set_selectable(true);
        let pane = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for w in [entry.upcast_ref::<gtk::Widget>(), list.upcast_ref(), grid.upcast_ref(), preview.upcast_ref()] {
            pane.append(w);
        }
        list.set_hexpand(true);
        let win = gtk::Window::new();
        win.set_default_size(500, 200);
        win.set_child(Some(&pane));
        win.present();
        pump();
        let focus = || gtk::prelude::GtkWindowExt::focus(&win);

        // Focus outside the file pane is never taken.
        entry.grab_focus();
        pump();
        let outside = focus();
        assert!(outside.is_some() && !focus_within(&win, &list));
        let was_in = focus_within(&win, &list);
        store.remove_all();
        keep_focus_on_empty_view(&list, was_in, true);
        pump();
        assert_eq!(focus(), outside, "focus outside the file view is not stolen");

        // Details/Compact: focus on a row, last row removed.
        store.append(&gtk::StringObject::new("only-row"));
        pump();
        list.grab_focus();
        pump();
        assert!(focus_within(&win, &list) && focus().as_ref() != Some(list.upcast_ref()), "focus is on a row");
        let was_in = focus_within(&win, &list);
        store.remove_all();
        keep_focus_on_empty_view(&list, was_in, true);
        pump();
        assert_eq!(focus(), Some(list.clone().upcast()), "empty list root owns focus, no critical");

        // A non-empty result leaves focus alone.
        store.append(&gtk::StringObject::new("kept"));
        pump();
        list.grab_focus();
        pump();
        let before = focus();
        keep_focus_on_empty_view(&list, true, false);
        assert_eq!(focus(), before);

        // Grid: the FlowBox is the root.
        let child = grid_label.parent().expect("flow box child");
        child.grab_focus();
        pump();
        assert!(focus_within(&win, &grid) && focus().as_ref() != Some(grid.upcast_ref()));
        let was_in = focus_within(&win, &grid);
        while let Some(c) = grid.first_child() {
            grid.remove(&c);
        }
        keep_focus_on_empty_view(&grid, was_in, true);
        pump();
        assert_eq!(focus(), Some(grid.clone().upcast()), "empty grid root owns focus, no critical");
        win.close();
        pump();
    }

    /// Real pointer events (XTEST, on the virtual display only) through the
    /// production `connect_file_drag_with` controllers on a real ListView row.
    fn real_pointer_drag_carries_the_press_time_selection() {
        let xdo = |args: &[&str]| {
            let out = std::process::Command::new("xdotool").args(args).output().expect("xdotool");
            assert!(out.status.success(), "xdotool {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let paths = ["/d/a", "/d/b", "/d/c"];
        let store = gio::ListStore::new::<gtk::StringObject>();
        for p in paths {
            store.append(&gtk::StringObject::new(p));
        }
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let live_paths = {
            let (selection, store) = (selection.clone(), store.clone());
            Rc::new(move || -> Vec<String> {
                let set = selection.selection();
                (0..store.n_items())
                    .filter(|i| set.contains(*i))
                    .filter_map(|i| store.item(i).and_downcast::<gtk::StringObject>())
                    .map(|o| o.string().to_string())
                    .collect()
            })
        };
        let began: Rc<RefCell<Option<(Vec<String>, Vec<String>)>>> = Rc::new(RefCell::new(None));
        let rows: Rc<RefCell<Vec<gtk::Widget>>> = Rc::new(RefCell::new(Vec::new()));
        let factory = gtk::SignalListItemFactory::new();
        {
            let (live_paths, began, rows, selection) =
                (live_paths.clone(), began.clone(), rows.clone(), selection.clone());
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let path = item.item().and_downcast::<gtk::StringObject>().unwrap().string().to_string();
                let row = gtk::Box::new(gtk::Orientation::Horizontal, ROW_GAP);
                let name = gtk::Label::new(Some(&path));
                name.set_hexpand(true);
                name.set_xalign(0.0);
                let size = gtk::Label::new(Some("1 KB"));
                row.append(&name);
                row.append(&size);
                row.set_height_request(40);
                let selected: Rc<dyn Fn() -> Vec<String>> = {
                    let live_paths = live_paths.clone();
                    Rc::new(move || live_paths())
                };
                let drag = connect_file_drag_with(&row, selected, path, Some(&size));
                let (live_paths, began) = (live_paths.clone(), began.clone());
                drag.connect_drag_begin(move |_, gdk_drag| {
                    let provider = gdk_drag.content();
                    let carried = match provider.value(gdk::FileList::static_type()) {
                        Ok(v) => v
                            .get::<gdk::FileList>()
                            .map(|l| {
                                l.files()
                                    .iter()
                                    .filter_map(|f| f.path())
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                        Err(e) => {
                            eprintln!("wf-test: content value error {e}");
                            Vec::new()
                        }
                    };
                    *began.borrow_mut() = Some((carried, live_paths()));
                });
                // The ordinary press handling: a plain press selects only its row.
                let ordinary = gtk::GestureClick::new();
                let (sel, pos) = (selection.clone(), item.position());
                ordinary.connect_pressed(move |_, _, _, _| {
                    sel.select_item(pos, true);
                });
                row.add_controller(ordinary);
                rows.borrow_mut().push(row.clone().upcast());
                item.set_child(Some(&row));
            });
        }
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        let win = gtk::Window::new();
        win.set_title(Some("wf-drag-proof"));
        win.set_default_size(400, 240);
        win.set_child(Some(&list));
        win.present();
        pump();
        selection.select_item(0, true);
        selection.select_item(1, false);
        assert_eq!(live_paths(), vec!["/d/a".to_string(), "/d/b".to_string()]);
        let geo = xdo(&["search", "--name", "wf-drag-proof", "getwindowgeometry", "--shell"]);
        let num = |key: &str| -> i32 {
            geo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse().ok()).expect(key)
        };
        let (wx, wy) = (num("X="), num("Y="));
        let row_b = rows.borrow().iter().find(|r| {
            r.first_child().and_downcast::<gtk::Label>().is_some_and(|l| l.text() == "/d/b")
        }).cloned().expect("row b realized");
        let b = row_b.compute_bounds(&win).expect("row b bounds");
        let (px, py) = (wx + 30, wy + b.y() as i32 + 20);
        let mv = |x: i32, y: i32| {
            xdo(&["mousemove", &x.to_string(), &y.to_string()]);
            pump();
        };
        mv(px, py);
        xdo(&["mousedown", "1"]);
        pump();
        mv(px + 5, py);
        pump();
        mv(px + 40, py + 6);
        pump();
        mv(px + 60, py + 10);
        for _ in 0..5 {
            pump();
            if began.borrow().is_some() {
                break;
            }
        }
        let outcome = began.borrow().clone();
        xdo(&["mouseup", "1"]);
        pump();
        let (carried, live_at_begin) = outcome.expect("the real drag began");
        eprintln!("wf-test: live selection at drag begin = {live_at_begin:?}");
        assert_eq!(carried, vec!["/d/a".to_string(), "/d/b".to_string()], "prepared list keeps both");
        win.close();
        pump();
    }

    fn name_dialog_ok_and_enter_invoke_callback_once_cancel_never() {
        let parent = gtk::Window::new();
        parent.present();
        pump();
        let build = |calls: &Rc<RefCell<Vec<String>>>| {
            let calls = calls.clone();
            let built = build_name_dialog("Rename", "a.txt", move |n| calls.borrow_mut().push(n));
            built.0.present(Some(&parent));
            pump();
            built
        };

        let calls = Rc::new(RefCell::new(Vec::new()));
        let (dialog, entry) = build(&calls);
        entry.set_text("b.txt");
        dialog.emit_by_name::<()>("response", &[&"ok"]);
        assert_eq!(*calls.borrow(), vec!["b.txt".to_string()], "OK reaches the callback once");

        let calls = Rc::new(RefCell::new(Vec::new()));
        let (dialog, entry) = build(&calls);
        entry.set_text("c.txt");
        dialog.emit_by_name::<()>("response", &[&"cancel"]);
        assert!(calls.borrow().is_empty(), "Cancel never calls back");

        let calls = Rc::new(RefCell::new(Vec::new()));
        let (_dialog, entry) = build(&calls);
        entry.set_text("d.txt");
        entry.emit_activate();
        assert_eq!(*calls.borrow(), vec!["d.txt".to_string()], "Enter reaches the callback once");
    }

    #[test]
    fn activation_gate_single_and_ctrl_click_never_open() {
        use std::time::Instant;
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut g = ActivationGate::default();
        // one plain click, then a stray position-only activation
        g.press("/d/a", ms(0), true);
        assert!(!g.allows("/d/a", ms(10)));
        // a click on another file, then an activation naming the first: refused
        g.press("/d/b", ms(100), true);
        g.press("/d/a", ms(200), true);
        assert!(!g.allows("/d/a", ms(210)), "the presses were on different files");
        // real double-click on one file opens once
        g.press("/d/a", ms(1000), true);
        g.press("/d/a", ms(1150), true);
        assert!(g.allows("/d/a", ms(1155)));
        assert!(!g.allows("/d/a", ms(1160)), "consumed");
        // two slow presses do not open
        g.press("/d/a", ms(3000), true);
        g.press("/d/a", ms(4000), true);
        assert!(!g.allows("/d/a", ms(4005)));
        // Ctrl-click twice, Shift-click twice, or one modified press: never opens
        g.press("/d/a", ms(8000), false);
        g.press("/d/a", ms(8100), false);
        assert!(!g.allows("/d/a", ms(8110)), "double Ctrl-click only selects");
        g.press("/d/a", ms(8200), false);
        g.press("/d/a", ms(8300), true);
        assert!(!g.allows("/d/a", ms(8310)), "one modified press refuses");
        g.press("/d/a", ms(8400), true);
        g.press("/d/a", ms(8500), false);
        assert!(!g.allows("/d/a", ms(8510)), "modified second press refuses");
        // Enter opens, once, only when fresh
        g.enter(ms(5000));
        assert!(g.allows("/d/x", ms(5050)));
        assert!(!g.allows("/d/x", ms(5060)));
        g.enter(ms(6000));
        assert!(!g.allows("/d/x", ms(7000)), "stale Enter");
    }

    fn list_activation_gate_on_real_listview_opens_only_when_allowed() {
        let list = gtk::ListView::new(None::<gtk::SelectionModel>, None::<gtk::ListItemFactory>);
        let gate = Rc::new(RefCell::new(ActivationGate::default()));
        let opened = Rc::new(RefCell::new(Vec::<String>::new()));
        let path_at: Rc<dyn Fn(u32) -> Option<String>> =
            Rc::new(|p| Some(format!("/d/file{p}")));
        let open: Rc<dyn Fn(String)> = {
            let opened = opened.clone();
            Rc::new(move |p| opened.borrow_mut().push(p))
        };
        connect_gated_activate(&list, gate.clone(), path_at, open);
        let started = std::time::Instant::now();
        list.emit_by_name::<()>("activate", &[&0u32]);
        assert!(opened.borrow().is_empty(), "a bare activation opens nothing");
        gate.borrow_mut().press("/d/file1", std::time::Instant::now(), true);
        gate.borrow_mut().press("/d/file1", std::time::Instant::now(), true);
        list.emit_by_name::<()>("activate", &[&1u32]);
        assert_eq!(*opened.borrow(), vec!["/d/file1".to_string()]);
        gate.borrow_mut().press("/d/file1", std::time::Instant::now(), true);
        list.emit_by_name::<()>("activate", &[&2u32]);
        assert_eq!(opened.borrow().len(), 1, "a click on one file never opens another");
        assert!(started.elapsed() < Duration::from_millis(500), "activation returns promptly");
    }

    fn pump() {
        let ctx = glib::MainContext::default();
        for _ in 0..40 {
            while ctx.iteration(false) {}
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Real GTK widgets and real Scratch paths: focus loss and an outside press
    /// never write; Enter writes exactly once; no watcher is left behind.
    fn inline_editor_click_away_never_writes_and_enter_writes_once() {
        let root = std::path::Path::new(&std::env::var("HOME").unwrap())
            .join(format!("Projects/Scratch/wf-inline-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::create_dir_all(&root).unwrap();
        let orig = root.join("rename-me.txt").to_string_lossy().to_string();
        let proposed = root.join("changed name.txt").to_string_lossy().to_string();
        std::fs::write(&orig, b"ORIGINAL-BYTES").unwrap();

        let window = gtk::Window::new();
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let field = gtk::Entry::builder().text("rename-me.txt").build();
        let outside = gtk::Button::with_label("outside");
        holder.append(&field);
        holder.append(&outside);
        window.set_child(Some(&holder));
        window.set_default_size(300, 200);

        let commits = Rc::new(RefCell::new(0));
        let cancels = Rc::new(RefCell::new(0));
        let live = Rc::new(RefCell::new(true));
        let controllers_before = window.observe_controllers().n_items();
        wire_inline_editor(
            &field,
            &window,
            InlineHost {
                commit: {
                    let (commits, orig) = (commits.clone(), orig.clone());
                    Rc::new(move |f| {
                        *commits.borrow_mut() += 1;
                        native_core::rename_path(&orig, f.text().trim()).unwrap();
                    })
                },
                cancel: {
                    let (cancels, live) = (cancels.clone(), live.clone());
                    Rc::new(move || {
                        *cancels.borrow_mut() += 1;
                        *live.borrow_mut() = false;
                    })
                },
                active: {
                    let live = live.clone();
                    Rc::new(move || *live.borrow())
                },
            },
        );
        window.present();
        pump();
        field.grab_focus();
        pump();
        field.set_text("changed name.txt");

        // focus loss
        outside.set_focusable(true);
        outside.grab_focus();
        pump();
        assert_eq!(*commits.borrow(), 0, "focus loss must not commit");
        assert!(*cancels.borrow() >= 1, "focus loss cancels");
        assert_eq!(std::fs::read(&orig).unwrap(), b"ORIGINAL-BYTES");
        assert!(!std::path::Path::new(&proposed).exists());

        // outside press through the window watcher
        *live.borrow_mut() = true;
        field.grab_focus();
        pump();
        let watcher = window
            .observe_controllers()
            .into_iter()
            .filter_map(|o| o.ok()?.downcast::<gtk::GestureClick>().ok())
            .find(|g| g.propagation_phase() == gtk::PropagationPhase::Capture)
            .expect("watcher installed");
        let p = outside
            .compute_point(&window, &gtk::graphene::Point::new(3.0, 3.0))
            .expect("point");
        let before = *cancels.borrow();
        watcher.emit_by_name::<()>("pressed", &[&1i32, &(p.x() as f64), &(p.y() as f64)]);
        pump();
        assert!(*cancels.borrow() > before || !*live.borrow(), "outside press cancels");
        assert_eq!(*commits.borrow(), 0, "outside press must not commit");
        assert_eq!(std::fs::read(&orig).unwrap(), b"ORIGINAL-BYTES");
        assert!(!std::path::Path::new(&proposed).exists());
        assert_eq!(
            window.observe_controllers().n_items(),
            controllers_before + 0,
            "the watcher is removed after the outside press"
        );

        // Enter: exactly one rename
        field.emit_activate();
        pump();
        assert_eq!(*commits.borrow(), 1);
        assert!(!std::path::Path::new(&orig).exists());
        assert_eq!(std::fs::read(&proposed).unwrap(), b"ORIGINAL-BYTES");
    }

    #[test]
    fn inline_rename_commit_guard_writes_once_and_failure_keeps_text() {
        let mut edit = InlineRename::new("/d/a.txt", "a.txt");
        assert!(edit.begin_commit("b.txt"), "Enter claims the commit");
        assert!(!edit.begin_commit("b.txt"), "the focus loss after Enter must not write again");
        edit.fail("\"b.txt\" already exists".into());
        assert_eq!(edit.text, "b.txt", "text kept for the user to fix");
        assert!(edit.error.is_some());
        assert!(edit.begin_commit("c.txt"), "a failed commit can be retried");
    }

    #[test]
    fn only_enter_commits_click_away_and_escape_cancel() {
        assert!(inline_event_commits(InlineEvent::Enter));
        assert!(!inline_event_commits(InlineEvent::FocusLoss), "click-away must not write");
        assert!(!inline_event_commits(InlineEvent::Escape));
    }

    #[test]
    fn f2_needs_exactly_one_selected_row_in_a_list_view() {
        assert!(f2_starts_inline_rename(1, ViewMode::Details));
        assert!(f2_starts_inline_rename(1, ViewMode::Compact));
        assert!(!f2_starts_inline_rename(0, ViewMode::Details));
        assert!(!f2_starts_inline_rename(2, ViewMode::Details), "several selected stays batch rename");
        assert!(!f2_starts_inline_rename(1, ViewMode::Grid), "grid is not in this round");
    }

    #[test]
    fn slow_click_needs_an_already_selected_name() {
        let slow = Some(Duration::from_millis(900));
        assert!(slow_click_starts_rename(slow, 1, true));
        assert!(!slow_click_starts_rename(None, 1, true), "first click only selects");
        assert!(!slow_click_starts_rename(slow, 2, true), "double-click opens");
        assert!(!slow_click_starts_rename(Some(Duration::from_millis(150)), 1, true), "too fast");
        assert!(!slow_click_starts_rename(Some(Duration::from_secs(5)), 1, true), "too old");
        assert!(!slow_click_starts_rename(slow, 1, false), "not the only selection");
    }

    #[test]
    fn rename_selects_the_name_but_not_the_extension() {
        // The whole point: typing a new name must not eat ".png".
        assert_eq!(rename_selection_end("digital-duck.png", false), 12);
        assert_eq!(rename_selection_end("archive.tar.gz", false), 11, "only the last suffix is the extension");
        // Nothing to protect -> select everything.
        assert_eq!(rename_selection_end("Downloads", true), -1, "folders have no extension");
        assert_eq!(rename_selection_end("README", false), -1, "no extension");
        assert_eq!(rename_selection_end(".bashrc", false), -1, "a leading dot is not an extension");
        // select_region counts characters, not bytes.
        assert_eq!(rename_selection_end("café.txt", false), 4, "must be chars, not bytes");
    }

    #[test]
    fn name_sort_is_numeric_aware() {
        let mut v = vec![
            entry("file10.txt", false, 0),
            entry("file2.txt", false, 0),
            entry("File1.txt", false, 0),
        ];
        sort_entries(&mut v, SortKey::Name, true);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["File1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn desktop_folder_argument_reaches_gapplication() {
        let args = vec![
            "wayfinder".to_string(),
            "file:///home/zito/Desktop/Projects".to_string(),
        ];
        assert_eq!(gtk_launch_args(&args), args);
    }

    #[test]
    fn picker_arguments_stay_out_of_gapplication() {
        let args = vec![
            "wayfinder".to_string(),
            "--pick".to_string(),
            "--out".to_string(),
            "/tmp/selection".to_string(),
        ];
        assert_eq!(gtk_launch_args(&args), vec!["wayfinder"]);
    }

    /// D-19: two windows share one config file. A window changing its view
    /// settings must not undo another window's favorites or recents, and
    /// concurrent favorite/recent updates must both survive.
    #[test]
    fn config_updates_preserve_fields_changed_by_another_window_d19() {
        let dir = crate::native_core::scratch_root().join(format!("wf-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _cleanup = ScratchCleanup(dir.clone());
        let path = dir.join("native-config.json");
        let (d1, d2) = (dir.join("one"), dir.join("two"));
        fs::create_dir_all(&d1).unwrap();
        fs::create_dir_all(&d2).unwrap();
        let (p1, p2) = (d1.to_string_lossy().to_string(), d2.to_string_lossy().to_string());
        update_config_at(&path, |c| c.favorites.clear());
        // window A and window B both start from the same (empty) view of the file
        update_config_at(&path, |c| add_favorite(&mut c.favorites, &p1)); // A
        update_config_at(&path, |c| add_favorite(&mut c.favorites, &p2)); // B, stale
        update_config_at(&path, |c| push_recent(&mut c.recents, &p1));
        update_config_at(&path, |c| push_recent(&mut c.recents, &p2));
        // B then changes only a view setting
        update_config_at(&path, |c| {
            c.show_hidden = true;
            c.sort_ascending = false;
        });
        let c = read_config_at(&path);
        let fav: Vec<_> = c.favorites.iter().map(|f| f.path.clone()).collect();
        assert_eq!(fav, vec![p1.clone(), p2.clone()], "both favorites survive");
        let rec: Vec<_> = c.recents.iter().map(|f| f.path.clone()).collect();
        assert_eq!(rec, vec![p2.clone(), p1.clone()], "both recents survive");
        assert!(c.show_hidden && !c.sort_ascending);
        update_config_at(&path, |c| remove_favorite(&mut c.favorites, &p1));
        assert_eq!(read_config_at(&path).favorites.len(), 1);
        // D-35: clearing recents touches only recents, and a later visit reads the cleared file.
        update_config_at(&path, |c| c.recents.clear());
        let c = read_config_at(&path);
        assert!(c.recents.is_empty() && c.favorites.len() == 1 && c.show_hidden && !c.sort_ascending);
        update_config_at(&path, |c| push_recent(&mut c.recents, &p1));
        let rec: Vec<_> = read_config_at(&path).recents.iter().map(|f| f.path.clone()).collect();
        assert_eq!(rec, vec![p1.clone()], "only the new visit follows a clear");
        let _ = fs::remove_dir_all(&dir);
    }

    /// D-19 correction: each view control persists only its own field. A stale
    /// window changing one setting must not write back another window's change.
    #[test]
    fn view_settings_persist_only_the_changed_field_d19() {
        let dir = crate::native_core::scratch_root().join(format!("wf-view-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _cleanup = ScratchCleanup(dir.clone());
        let path = dir.join("native-config.json");
        update_config_at(&path, |c| *c = AppConfig::default());
        // A changes every field; stale B (which still thinks the defaults hold)
        // then changes one field at a time.
        update_config_at(&path, |c| config_show_hidden(c, true));
        update_config_at(&path, |c| config_view_mode(c, ViewMode::Grid));
        update_config_at(&path, |c| config_search_in_files(c, true));
        update_config_at(&path, |c| config_sort(c, SortKey::Size, false));
        let want = read_config_at(&path);
        assert!(want.show_hidden && want.search_in_files && !want.sort_ascending);
        // stale B: sort only
        update_config_at(&path, |c| config_sort(c, SortKey::Modified, true));
        let c = read_config_at(&path);
        assert!(c.show_hidden && c.search_in_files, "sort must not touch hidden/search");
        assert_eq!(c.view_mode, want.view_mode, "sort must not touch view mode");
        assert_eq!((c.sort_key, c.sort_ascending), (SortKey::Modified, true));
        // stale B: hidden only
        update_config_at(&path, |c| config_show_hidden(c, false));
        let c = read_config_at(&path);
        assert!(!c.show_hidden && c.search_in_files);
        assert_eq!((c.sort_key, c.sort_ascending), (SortKey::Modified, true));
        assert_eq!(c.view_mode, want.view_mode);
        // stale B: search only
        update_config_at(&path, |c| config_search_in_files(c, false));
        let c = read_config_at(&path);
        assert!(!c.search_in_files && !c.show_hidden);
        assert_eq!(c.view_mode, want.view_mode);
        // stale B: view mode only
        update_config_at(&path, |c| config_show_hidden(c, true));
        update_config_at(&path, |c| config_view_mode(c, ViewMode::Compact));
        let c = read_config_at(&path);
        assert!(c.show_hidden && !c.search_in_files);
        assert_eq!((c.sort_key, c.sort_ascending), (SortKey::Modified, true));
        assert_ne!(c.view_mode, want.view_mode);
        assert!(!include_str!("native.rs").contains(concat!("save_view", "_settings(")), "no whole-snapshot view write");
        let _ = fs::remove_dir_all(&dir);
    }

    /// D-20: AdwAlertDialog closes itself after `response`; closing it again in
    /// the handler raises a GTK critical.
    #[test]
    fn conflict_dialog_response_does_not_close_again_d20() {
        let all = include_str!("native.rs");
        let start = all.find("fn show_conflict_dialog<F>").unwrap();
        let body = &all[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(body.contains("connect_response"));
        assert!(!body.contains(".close()"), "response handler must not close the dialog again");
    }

    #[test]
    fn ctrl_n_opens_a_window_and_leaves_the_other_shortcuts_d19() {
        assert!(is_new_window_shortcut(true, false, false, gdk::Key::n));
        assert!(!is_new_window_shortcut(true, true, false, gdk::Key::N), "Ctrl+Shift+N is New Folder");
        assert!(!is_new_window_shortcut(true, false, false, gdk::Key::t), "Ctrl+T is New Tab");
        assert!(!is_new_window_shortcut(false, false, false, gdk::Key::n));
        let all = include_str!("native.rs");
        assert!(all.contains("open_new_window(&ui_for_keys, state_for_keys.borrow().cwd.clone())"));
        assert!(all.contains("\"Open in New Window\""));
    }

    /// D-18 (23 Sep 2026): the drag picture is a compact translucent ghost of the
    /// dragged item's icon and name, never the whole row. Code cannot prove the
    /// name beneath is readable; only eyes can.
    #[test]
    fn drag_ghost_is_compact_icon_and_name_d18() {
        assert!((0.50..=0.60).contains(&DRAG_GHOST_OPACITY));
        assert!(DRAG_GHOST_MAX_WIDTH <= 240);
        let all = include_str!("native.rs");
        let app = &all[..all.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!app.contains("WidgetPaintable") && !app.contains("DRAG_HAND"));
        assert_eq!(app.matches("set_opacity(").count(), 1, "only the ghost is translucent");
        assert!(app.contains("ghost.set_opacity(DRAG_GHOST_OPACITY)"));
        assert!(app.contains("drag.set_hotspot(hot_x, hot_y)"));
        assert!(app.contains("ghost_hotspot(grab.get(), &ghost)"));
        // hotspot clamped into the ghost, never negative
        assert_eq!(clamp_hotspot((500.0, 300.0), 200, 30), (199, 29));
        assert_eq!(clamp_hotspot((-5.0, -1.0), 200, 30), (0, 0));
        assert_eq!(clamp_hotspot((40.4, 12.6), 200, 30), (40, 13));
    }

    fn drag_ghost_builds_compact_translucent_item() {
        let long = "a-very-long-file-name-that-would-never-fit-in-a-compact-ghost-at-all.txt";
        let (overlay, ghost) = build_drag_ghost(&format!("/tmp/{long}"), 3);
        let (min_w, nat_w, _, _) = ghost.measure(gtk::Orientation::Horizontal, -1);
        assert!(nat_w <= DRAG_GHOST_MAX_WIDTH && min_w <= DRAG_GHOST_MAX_WIDTH, "ghost {nat_w}px wide");
        let (_, nat_h, _, _) = ghost.measure(gtk::Orientation::Vertical, -1);
        assert!(nat_h <= 48, "ghost {nat_h}px high");
        assert!((ghost.opacity() - DRAG_GHOST_OPACITY).abs() < 1e-3);
        // the count badge sits beside the ghost in the overlay, opaque
        let mut badge = None;
        let mut child = overlay.first_child();
        while let Some(c) = child {
            if c.has_css_class("wf-drag-badge") {
                badge = Some(c.clone());
            }
            child = c.next_sibling();
        }
        let badge = badge.expect("count badge present for 3 items");
        assert!((badge.opacity() - 1.0).abs() < 1e-3);
        assert!(badge.parent().as_ref() == Some(overlay.upcast_ref::<gtk::Widget>()));
        let (o1, g1) = build_drag_ghost("/tmp/x.txt", 1);
        assert!(o1.first_child().is_some() && g1.opacity() < 1.0);
    }

    /// LOCKED 12 Sep 2026: dragging a file from anywhere on its row must beat the
    /// list's selection box. Zito confirmed it works. If this fails, a change
    /// removed part of that fix — put it back, do not edit this test.
    #[test]
    fn row_drag_beats_selection_box_locked() {
        // Only the app code above the tests, or these strings would match themselves.
        let full = include_str!("native.rs");
        let src = &full[..full.find(concat!("#[cfg", "(test)]")).expect("test module marker")];
        for piece in [
            "claim.set_propagation_phase(gtk::PropagationPhase::Capture);",
            "claim.group_with(&drag);",
            "gtk_dnd_drag_threshold() as f64 / 2.0",
            "gesture.set_state(gtk::EventSequenceState::Claimed);",
            "if strip_for_prepare(x) {",
            "drag.set_propagation_phase(gtk::PropagationPhase::Capture);",
            "connect_file_drag(&line, state.clone(), ui.clone(), entry.path.clone(), Some(&size));",
            "const BOX_SELECT_STRIP_W: f64 = 24.0;",
            "padding: 4px 12px;",
        ] {
            assert!(src.contains(piece), "locked row-drag fix is missing: {piece}");
        }
        assert!(
            !src.contains(concat!("line.set_", "margin_top(2);")),
            "row margins are back: they leave dead space that starts a selection box"
        );
    }
}
