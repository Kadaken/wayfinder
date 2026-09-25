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

fn main() {
    if std::env::args().any(|arg| arg == "--pick-capable") {
        std::process::exit(0);
    }

    let app = adw::Application::builder()
        .application_id("io.github.legendofzito.wayfinder")
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
                    open_entry(&state, &ui, entry);
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
                open_entry(&state, &ui, entry);
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

    let drives = native_core::list_drives();
    if !drives.is_empty() {
        append_sidebar_heading(&ui.sidebar, "Devices");
        for drive in drives {
            let detail = if drive.mountpoint.is_empty() {
                "mount".to_string()
            } else if drive.size > 0 {
                native_core::fmt_size(drive.size)
            } else {
                String::new()
            };
            let row = sidebar_row(&drive.name, &detail, drive_icon(&drive));
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

    if let Some(drive) = native_core::list_drives()
        .into_iter()
        .find(|drive| drive.mountpoint == path || drive.path == path)
    {
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

fn append_sidebar_heading(sidebar: &gtk::ListBox, text: &str) {
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
}

fn sidebar_row(name: &str, detail: &str, icon_name: &str) -> gtk::ListBoxRow {
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
    detail.set_max_width_chars(12);
    detail.add_css_class("wf-sidebar-detail");
    text.append(&name);
    if !detail.label().is_empty() {
        text.append(&detail);
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
            clear_file_views(ui);
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
        if let Err(err) = native_core::open_path(&entry.path) {
            ui.status.set_label(&format!("Open failed: {err}"));
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
    if let Err(err) = native_core::open_terminal(&cwd) {
        ui.status.set_label(&format!("Terminal failed: {err}"));
    }
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
        |state, ui, _| rebuild_sidebar(state, ui),
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
        |state, ui, _| rebuild_sidebar(state, ui),
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
        |state, ui, _| rebuild_sidebar(state, ui),
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
        menu.append(&context_button(label, &popover, move || {
            open_drive_action(state.clone(), ui.clone(), drive.clone())
        }));
    }

    if drive.kind == "drive" && !drive.mountpoint.is_empty() {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        menu.append(&context_item("media-eject-symbolic", "Unmount", "", &popover, move || {
            unmount_drive_action(state.clone(), ui.clone(), drive.clone())
        }));
    }

    if drive.removable {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        menu.append(&context_item("media-eject-symbolic", "Eject", "", &popover, move || {
            eject_drive_action(state.clone(), ui.clone(), drive.clone())
        }));
    }

    popover.set_child(Some(&menu));
    popover.popup();
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
            let popover = popover.clone();
            button.connect_clicked(move |_| {
                popover.popdown();
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
        menu.append(&context_item("application-x-executable-symbolic", "Open With...", "", &popover, move || {
            show_open_with_dialog(ui.clone(), path.clone())
        }));
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
            if let Err(err) = native_core::open_terminal(&terminal_path) {
                ui.status.set_label(&format!("Terminal failed: {err}"));
            }
        }));
    }

    popover.set_child(Some(&menu));
    popover.popup();
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
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        popover.popdown();
        action();
    });
    button
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
    let parent = popover.clone();
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
                parent.popdown();
                action();
            }));
        }
        side.set_child(Some(&menu));
        side.connect_closed(|side| {
            let side = side.clone();
            glib::idle_add_local_once(move || side.unparent());
        });
        side.popup();
    });
    button
}

fn context_separator() -> gtk::Separator {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("wf-menu-sep");
    sep
}

#[allow(deprecated)]
fn show_open_with_dialog(ui: Ui, path: String) {
    let apps = native_core::open_with_apps(&path);
    if apps.is_empty() {
        ui.status.set_label("No Open With applications found");
        return;
    }

    let dialog = gtk::Dialog::with_buttons(
        Some("Open With"),
        Some(&ui.window),
        gtk::DialogFlags::MODAL,
        &[("Cancel", gtk::ResponseType::Cancel)],
    );
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(16);
    content.set_margin_end(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);

    for app in apps {
        let button = gtk::Button::builder()
            .label(&app.name)
            .halign(gtk::Align::Fill)
            .build();
        let ui_for_app = ui.clone();
        let dialog_for_app = dialog.clone();
        let path_for_app = path.clone();
        button.connect_clicked(move |_| {
            match native_core::open_with(&app.id, &path_for_app) {
                Ok(()) => ui_for_app
                    .status
                    .set_label(&format!("Opened with {}", app.name)),
                Err(err) => ui_for_app
                    .status
                    .set_label(&format!("Open With failed: {err}")),
            }
            dialog_for_app.close();
        });
        content.append(&button);
    }

    dialog.content_area().append(&content);
    dialog.present();
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
        let (_app, state, ui) = open("io.github.legendofzito.wayfinder.d30test");
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
        let (_app2, _state2, ui2) = open("io.github.legendofzito.wayfinder.d30test2");
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
            .application_id("io.github.legendofzito.wayfinder.d32test")
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
            .application_id("io.github.legendofzito.wayfinder.d29test")
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

        let (_app, state, ui) = open("io.github.legendofzito.wayfinder.d28test");
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
        let (_app2, state2, ui2) = open("io.github.legendofzito.wayfinder.d28test2");
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
            .application_id("io.github.legendofzito.wayfinder.d27test")
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
            .application_id("io.github.legendofzito.wayfinder.d22test")
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
            .application_id("io.github.legendofzito.wayfinder.d22stream")
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
            .application_id("io.github.legendofzito.wayfinder.d21test")
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
