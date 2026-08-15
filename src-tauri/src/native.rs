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

popover button.flat {
    color: @wf_text;
    border-radius: 6px;
    padding: 6px 10px;
}

popover button.flat:hover {
    background: @wf_accent;
    color: #ffffff;
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

fn save_config(state: &AppState) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let config = AppConfig {
        favorites: state.favorites.clone(),
        recents: state.recents.clone(),
        show_hidden: state.show_hidden,
        view_mode: view_mode_id(state.view_mode).to_string(),
        search_in_files: state.search_in_files,
        sort_key: state.sort_key,
        sort_ascending: state.sort_ascending,
    };
    if let Ok(raw) = serde_json::to_string_pretty(&config) {
        let _ = fs::write(path, raw);
    }
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

fn push_recent_dir(state: &mut AppState, path: &str) {
    if !Path::new(path).is_dir() {
        return;
    }
    state.recents.retain(|bookmark| bookmark.path != path);
    state.recents.insert(0, bookmark_for_path(path));
    state.recents.truncate(12);
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
            state_ref.favorites.push(bookmark_for_path(&path));
            save_config(&state_ref);
        }
    }
    rebuild_sidebar(state, ui);
    ui.status.set_label("Added to Favorites");
}

fn remove_favorite_path(state: &Rc<RefCell<AppState>>, ui: &Ui, path: String) {
    {
        let mut state_ref = state.borrow_mut();
        state_ref.favorites.retain(|favorite| favorite.path != path);
        save_config(&state_ref);
    }
    rebuild_sidebar(state, ui);
    ui.status.set_label("Removed from Favorites");
}

#[derive(Clone)]
struct AppState {
    cwd: String,
    tabs: Vec<String>,
    active_tab: usize,
    history: Vec<String>,
    history_index: usize,
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
    monitor_refresh_generation: u64,
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
    });
    app.connect_activate(|app| {
        let existing_ui = CURRENT_UI.with(|current| current.borrow().clone());
        if let Some(ui) = existing_ui {
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
        let existing = CURRENT_STATE.with(|current| current.borrow().clone());
        let existing_ui = CURRENT_UI.with(|current| current.borrow().clone());
        if let (Some(state), Some(ui)) = (existing, existing_ui) {
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
        cwd: initial_cwd.clone(),
        tabs: vec![initial_cwd.clone()],
        active_tab: 0,
        history: vec![initial_cwd],
        history_index: 0,
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
        monitor_refresh_generation: 0,
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
    path_bar.append(&details_view);
    path_bar.append(&compact_view);
    path_bar.append(&icon_view);
    path_bar.append(&refresh);
    path_bar.append(&options);

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
    APP_HOLD.with(|hold| *hold.borrow_mut() = Some(app.hold()));
    ui.window.connect_close_request(move |_| {
        APP_HOLD.with(|hold| {
            hold.borrow_mut().take();
        });
        glib::Propagation::Proceed
    });
    CURRENT_UI.with(|current| *current.borrow_mut() = Some(ui.clone()));
    CURRENT_STATE.with(|current| *current.borrow_mut() = Some(state.clone()));

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
            save_config(&state_ref);
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
        list.connect_activate(move |_, position| {
            let entry = state
                .borrow()
                .visible_entries
                .get(position as usize)
                .cloned();
            if let Some(entry) = entry {
                open_entry(&state, &ui, entry);
            }
        });
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
        (state.tabs.clone(), state.active_tab)
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
    let path = {
        let mut state = state.borrow_mut();
        if index >= state.tabs.len() {
            return;
        }
        state.active_tab = index;
        state.tabs[index].clone()
    };
    navigate(&state, path);
}

/// Open `path` in a new tab and switch to it, the way a middle-click or
/// "Open in New Tab" is expected to behave.
fn open_path_in_new_tab(state: &Rc<RefCell<AppState>>, ui: &Ui, path: String) {
    {
        let mut state = state.borrow_mut();
        state.tabs.push(path.clone());
        state.active_tab = state.tabs.len() - 1;
    }
    rebuild_tabs(state, ui);
    navigate(state, path);
}

fn new_tab_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let path = state.borrow().cwd.clone();
    {
        let mut state = state.borrow_mut();
        state.tabs.push(path.clone());
        state.active_tab = state.tabs.len() - 1;
    }
    rebuild_tabs(&state, &ui);
    navigate(&state, path);
}

fn close_tab_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let path = {
        let mut state = state.borrow_mut();
        if state.tabs.len() <= 1 {
            ui.status.set_label("Keep at least one tab open");
            return;
        }
        let active = state.active_tab;
        state.tabs.remove(active);
        if state.active_tab >= state.tabs.len() {
            state.active_tab = state.tabs.len() - 1;
        }
        state.tabs[state.active_tab].clone()
    };
    rebuild_tabs(&state, &ui);
    navigate(&state, path);
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
            let path = {
                let mut state = state.borrow_mut();
                if state.history_index == 0 {
                    return;
                }
                state.history_index -= 1;
                state.history[state.history_index].clone()
            };
            navigate_without_history(&state, path);
        });
    }
    {
        let state = state.clone();
        forward.connect_clicked(move |_| {
            let path = {
                let mut state = state.borrow_mut();
                if state.history_index + 1 >= state.history.len() {
                    return;
                }
                state.history_index += 1;
                state.history[state.history_index].clone()
            };
            navigate_without_history(&state, path);
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
        hidden.connect_toggled(move |button| {
            {
                let mut state = state.borrow_mut();
                state.show_hidden = button.is_active();
                save_config(&state);
            }
            let cwd = state.borrow().cwd.clone();
            navigate(&state, cwd);
        });
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

fn set_view_mode(state: &Rc<RefCell<AppState>>, ui: &Ui, mode: ViewMode) {
    {
        let mut state_ref = state.borrow_mut();
        state_ref.view_mode = mode;
        save_config(&state_ref);
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
                        save_config(&state_ref);
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
        menu.append(&context_button("New Folder", &popover, move || {
            create_folder_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("New File", &popover, move || {
            create_file_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Refresh", &popover, move || {
            let cwd = state.borrow().cwd.clone();
            rebuild_sidebar(&state, &ui);
            navigate_without_history(&state, cwd);
        }));
    }
    {
        let state = state.clone();
        menu.append(&context_button("Show Hidden Files", &popover, move || {
            {
                let mut state = state.borrow_mut();
                state.show_hidden = !state.show_hidden;
                save_config(&state);
            }
            let cwd = state.borrow().cwd.clone();
            navigate(&state, cwd);
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Rename", &popover, move || {
            rename_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Copy", &popover, move || {
            copy_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Cut", &popover, move || {
            cut_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Paste", &popover, move || {
            paste_clipboard(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Move to Trash", &popover, move || {
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
        menu.append(&context_button("Open Terminal Here", &popover, move || {
            terminal_here(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Empty Trash", &popover, move || {
            empty_trash_action(state.clone(), ui.clone())
        }));
    }

    popover.set_child(Some(&menu));
    popover.popup();
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
            let path = {
                let mut state = state_for_keys.borrow_mut();
                if state.history_index == 0 {
                    return glib::Propagation::Stop;
                }
                state.history_index -= 1;
                state.history[state.history_index].clone()
            };
            navigate_without_history(&state_for_keys, path);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Right {
            let path = {
                let mut state = state_for_keys.borrow_mut();
                if state.history_index + 1 >= state.history.len() {
                    return glib::Propagation::Stop;
                }
                state.history_index += 1;
                state.history[state.history_index].clone()
            };
            navigate_without_history(&state_for_keys, path);
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
            let path = {
                let mut state = state_for_keys.borrow_mut();
                if state.history_index == 0 {
                    return glib::Propagation::Stop;
                }
                state.history_index -= 1;
                state.history[state.history_index].clone()
            };
            navigate_without_history(&state_for_keys, path);
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
    navigate_to(state, path, true);
}

fn navigate_without_history(state: &Rc<RefCell<AppState>>, path: String) {
    navigate_to(state, path, false);
}

fn navigate_to(state: &Rc<RefCell<AppState>>, path: String, record_history: bool) {
    let show_hidden = state.borrow().show_hidden;
    let ui = current_ui_for_state(state);
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
            if state.borrow().listing_generation != generation {
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
                let active = state.active_tab;
                if let Some(tab) = state.tabs.get_mut(active) {
                    *tab = path.clone();
                }
                if record_history {
                    let history_index = state.history_index;
                    let should_push = state
                        .history
                        .get(history_index)
                        .map(|current| current != &path)
                        .unwrap_or(true);
                    if should_push {
                        state.history.truncate(history_index + 1);
                        state.history.push(path.clone());
                        state.history_index = state.history.len() - 1;
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
                push_recent_dir(&mut state, &path);
                save_config(&state);
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
        let refresh_generation = {
            let mut state_ref = state.borrow_mut();
            if state_ref.monitor_generation != monitor_generation || state_ref.cwd != watched_path {
                return;
            }
            state_ref.monitor_refresh_generation =
                state_ref.monitor_refresh_generation.wrapping_add(1);
            state_ref.monitor_refresh_generation
        };
        let state_weak = Rc::downgrade(&state);
        let ui = ui.clone();
        let watched_path = watched_path.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            let Some(state) = state_weak.upgrade() else {
                return;
            };
            let should_refresh = {
                let state_ref = state.borrow();
                state_ref.monitor_generation == monitor_generation
                    && state_ref.monitor_refresh_generation == refresh_generation
                    && state_ref.cwd == watched_path
            };
            if should_refresh {
                ui.status.set_label("Refreshing after a file change…");
                navigate_without_history(&state, watched_path);
            }
        });
    });
    state.borrow_mut().directory_monitor = Some(monitor);
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
    ui.back.set_sensitive(state.history_index > 0);
    ui.forward
        .set_sensitive(state.history_index + 1 < state.history.len());
}

thread_local! {
    static CURRENT_UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static CURRENT_STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
    static APP_HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

fn current_ui_for_state(_state: &Rc<RefCell<AppState>>) -> Option<Ui> {
    CURRENT_UI.with(|ui| ui.borrow().clone())
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
fn connect_list_factory(factory: &gtk::SignalListItemFactory, state: &Rc<RefCell<AppState>>, ui: &Ui) {
    let state = state.clone();
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
        line.set_margin_top(2);
        line.set_margin_end(10);
        line.set_margin_bottom(2);
        line.set_margin_start(10);
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
        let size_text = if entry.metadata_error.is_some() {
            "—".to_string()
        } else if entry.is_dir {
            String::new()
        } else {
            native_core::fmt_size(entry.size)
        };
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
        let modified_text = entry
            .metadata_error
            .as_ref()
            .map(|_| "unavailable".to_string())
            .unwrap_or_else(|| native_core::fmt_modified(entry.modified));
        let modified = gtk::Label::builder()
            .label(&modified_text)
            .xalign(0.0)
            .max_width_chars(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        modified.add_css_class("wf-file-meta");
        ui.columns.modified.add_widget(&modified);

        line.append(&icon);
        line.append(&name);
        line.append(&size);
        line.append(&kind);
        line.append(&modified);

        connect_file_drag(&line, state.clone(), ui.clone(), entry.path.clone());
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
    CURRENT_UI.with(|current| *current.borrow_mut() = Some(ui.clone()));

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

    // Only Grid still builds a widget per item; Details/Compact are served by the
    // virtualized ListView above and must not walk the whole listing here.
    // NOTE: Grid remains un-virtualized, so a huge folder is still slow in Icons view.
    if view_mode != ViewMode::Grid {
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
            grid_item.append(&name);
            grid_child.set_child(Some(&grid_item));
            connect_file_drag(&grid_child, state.clone(), ui.clone(), entry.path.clone());
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

fn select_index(state: &Rc<RefCell<AppState>>, ui: &Ui, index: usize) {
    match state.borrow().view_mode {
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
    } else if let Err(err) = native_core::open_path(&entry.path) {
        ui.status.set_label(&format!("Open failed: {err}"));
    }
}

fn create_folder_action(state: Rc<RefCell<AppState>>, ui: Ui) {
    let cwd = state.borrow().cwd.clone();
    let ui_for_action = ui.clone();
    show_name_dialog(&ui, "New folder", "New Folder", NameSelection::All, move |name| {
        let state = state.clone();
        let ui = ui_for_action.clone();
        let cwd = cwd.clone();
        run_job(
            state,
            ui,
            format!("Creating folder {name}"),
            move || {
                let path = native_core::make_dir(&cwd, &name)?;
                Ok(JobOutcome::undo(
                    format!("Created {path}"),
                    UndoAction::Created { paths: vec![path] },
                ))
            },
            |_, _, _| {},
        );
    });
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
    let conflicts = native_core::check_conflicts(&clipboard.paths, &cwd);
    if conflicts.is_empty() {
        paste_with_conflict_mode(state, ui, native_core::PasteConflict::KeepBoth);
        return;
    }
    // Explorer asks before it overwrites. This used to silently keep both, every
    // time, which quietly filled folders with "file (2)" copies.
    let ui_for_choice = ui.clone();
    show_conflict_dialog(&ui, &conflicts, move |mode| {
        paste_with_conflict_mode(state.clone(), ui_for_choice.clone(), mode);
    });
}

/// Replace / Skip / Keep Both, with Cancel as the escape.
fn show_conflict_dialog<F>(ui: &Ui, conflicts: &[String], on_choice: F)
where
    F: Fn(native_core::PasteConflict) + 'static,
{
    let count = conflicts.len();
    let heading = if count == 1 {
        format!("\"{}\" already exists here.", conflicts[0])
    } else {
        format!("{count} items already exist here.")
    };
    let detail = if count == 1 {
        "Replace it, skip it, or keep both?".to_string()
    } else {
        let sample: Vec<&str> = conflicts.iter().take(5).map(String::as_str).collect();
        let more = count.saturating_sub(sample.len());
        let listed = sample.join(", ");
        if more > 0 {
            format!("{listed}, and {more} more.\n\nReplace them, skip them, or keep both?")
        } else {
            format!("{listed}\n\nReplace them, skip them, or keep both?")
        }
    };

    let dialog = adw::AlertDialog::new(Some(&heading), Some(&detail));
    dialog.add_responses(&[
        ("cancel", "Cancel"),
        ("skip", "Skip"),
        ("keepboth", "Keep Both"),
        ("replace", "Replace"),
    ]);
    // Replacing destroys the existing file and undo cannot bring it back, so it
    // is marked destructive and is NOT the default.
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("keepboth"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |dialog, response| {
        match response {
            "replace" => on_choice(native_core::PasteConflict::Replace),
            "skip" => on_choice(native_core::PasteConflict::Skip),
            "keepboth" => on_choice(native_core::PasteConflict::KeepBoth),
            _ => {}
        }
        dialog.close();
    });
    dialog.present(Some(&ui.window));
}

fn paste_with_conflict_mode(
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    on_conflict: native_core::PasteConflict,
) {
    let Some(clipboard) = state.borrow().clipboard.clone() else {
        return;
    };
    let cwd = state.borrow().cwd.clone();
    let is_copy = clipboard.mode == ClipboardMode::Copy;
    let clear_after = clipboard.mode == ClipboardMode::Cut;
    let count = clipboard.paths.len();
    let label = match on_conflict {
        native_core::PasteConflict::Replace => format!("Pasting {count}, replacing existing"),
        native_core::PasteConflict::Skip => format!("Pasting {count}, skipping existing"),
        native_core::PasteConflict::KeepBoth => format!("Pasting {count}"),
    };

    run_job(
        state,
        ui,
        label,
        move || {
            let items = native_core::resolve_paste(&clipboard.paths, &cwd, is_copy, on_conflict)?;
            Ok(JobOutcome::undo(
                format!("Pasted {} item{}", items.len(), plural(items.len())),
                UndoAction::Pasted {
                    items,
                    was_copy: is_copy,
                },
            ))
        },
        move |state, _, _| {
            if clear_after {
                state.borrow_mut().clipboard = None;
            }
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
            let mountpoint = native_core::mount_drive(&device)?;
            Ok(JobOutcome::navigate(
                format!("Mounted at {mountpoint}"),
                mountpoint,
            ))
        },
        |state, ui, _| rebuild_sidebar(state, ui),
    );
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
            navigate(&state, target);
            ui.status.set_label(&outcome.message);
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

fn show_name_dialog<F>(
    ui: &Ui,
    title: &str,
    default_name: &str,
    selection: NameSelection,
    on_ok: F,
) where
    F: Fn(String) + 'static,
{
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(&ui.window),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("OK", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    let entry = gtk::Entry::builder()
        .text(default_name)
        .activates_default(true)
        .hexpand(true)
        .margin_top(16)
        .margin_end(16)
        .margin_bottom(16)
        .margin_start(16)
        .build();
    dialog.content_area().append(&entry);
    let entry_for_response = entry.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Ok {
            let value = entry_for_response.text().trim().to_string();
            if !value.is_empty() {
                on_ok(value);
            }
        }
        dialog.close();
    });
    dialog.present();
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
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(&ui.window),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            (confirm, gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Cancel);
    let label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .xalign(0.0)
        .margin_top(16)
        .margin_end(16)
        .margin_bottom(16)
        .margin_start(16)
        .build();
    dialog.content_area().append(&label);
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Ok {
            on_confirm();
        }
        dialog.close();
    });
    dialog.present();
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
        menu.append(&context_button("Unmount", &popover, move || {
            unmount_drive_action(state.clone(), ui.clone(), drive.clone())
        }));
    }

    if drive.removable {
        let state = state.clone();
        let ui = ui.clone();
        let drive = drive.clone();
        menu.append(&context_button("Eject", &popover, move || {
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

    {
        let state = state.clone();
        let ui = ui.clone();
        let entry = entry.clone();
        menu.append(&context_button("Open", &popover, move || {
            open_entry(&state, &ui, entry.clone())
        }));
    }
    if entry.is_dir {
        let state = state.clone();
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_button("Open in New Tab", &popover, move || {
            open_path_in_new_tab(&state, &ui, path.clone())
        }));
    }
    if !entry.is_dir {
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_button("Open With...", &popover, move || {
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
        menu.append(&context_button("Cut", &popover, move || {
            cut_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Copy", &popover, move || {
            copy_selection(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Copy Paths", &popover, move || {
            copy_paths_to_system_clipboard(&state, &ui)
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Paste", &popover, move || {
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
        menu.append(&context_button("Compress to ZIP", &popover, move || {
            compress_selection(state.clone(), ui.clone())
        }));
    }
    if native_core::is_archive_path(&entry.path) {
        let state = state.clone();
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_button("Extract Here", &popover, move || {
            extract_archive_action(state.clone(), ui.clone(), path.clone())
        }));
    }

    menu.append(&context_separator());
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Move to Trash", &popover, move || {
            trash_selection(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Delete Permanently", &popover, move || {
            permanent_delete_selection(state.clone(), ui.clone())
        }));
    }

    menu.append(&context_separator());
    {
        let ui = ui.clone();
        let path = entry.path.clone();
        menu.append(&context_button("Properties", &popover, move || {
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
        menu.append(&context_button("Open Terminal Here", &popover, move || {
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
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("New Folder", &popover, move || {
            create_folder_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("New File", &popover, move || {
            create_file_action(state.clone(), ui.clone())
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Paste", &popover, move || {
            paste_clipboard(state.clone(), ui.clone())
        }));
    }
    if state.borrow().cwd.contains("/Trash/files") {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Empty Trash", &popover, move || {
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
        menu.append(&context_button("Properties", &popover, move || {
            let cwd = state.borrow().cwd.clone();
            show_properties_for_path(ui.clone(), cwd);
        }));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        menu.append(&context_button("Open Terminal Here", &popover, move || {
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
    let button = gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Fill)
        .build();
    button.add_css_class("flat");
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        popover.popdown();
        action();
    });
    button
}

fn context_separator() -> gtk::Separator {
    gtk::Separator::new(gtk::Orientation::Horizontal)
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

fn connect_file_drag<W: IsA<gtk::Widget>>(
    widget: &W,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    path: String,
) {
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
    drag.connect_prepare(move |_, _, _| {
        let selected = selected_paths(&state, &ui);
        let paths = if selected.iter().any(|selected| selected == &path) {
            selected
        } else {
            vec![path.clone()]
        };
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
    widget.add_controller(drag);
}

fn connect_file_drop<W: IsA<gtk::Widget>>(widget: &W, state: Rc<RefCell<AppState>>, ui: Ui) {
    let state_for_drop = state.clone();
    let ui_for_drop = ui.clone();
    let drop = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    drop.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gdk::FileList>() else {
            return false;
        };
        let srcs: Vec<String> = files
            .files()
            .into_iter()
            .filter_map(|file| file.path())
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        if srcs.is_empty() {
            return false;
        }
        let cwd = state_for_drop.borrow().cwd.clone();
        let count = srcs.len();
        run_job(
            state_for_drop.clone(),
            ui_for_drop.clone(),
            format!("Copying {count} dropped item{}", plural(count)),
            move || {
                // A drop is not a deliberate paste, so never destroy on conflict.
                let items = native_core::resolve_paste(&srcs, &cwd, true, native_core::PasteConflict::KeepBoth)?;
                Ok(JobOutcome::undo(
                    format!("Copied {} item{}", items.len(), plural(items.len())),
                    UndoAction::Pasted {
                        items,
                        was_copy: true,
                    },
                ))
            },
            |_, _, _| {},
        );
        true
    });
    widget.add_controller(drop);
}

fn connect_file_drop_to_dir<W: IsA<gtk::Widget>>(
    widget: &W,
    state: Rc<RefCell<AppState>>,
    ui: Ui,
    dest_dir: String,
) {
    let drop = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    drop.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gdk::FileList>() else {
            return false;
        };
        let srcs: Vec<String> = files
            .files()
            .into_iter()
            .filter_map(|file| file.path())
            .map(|path| path.to_string_lossy().to_string())
            .filter(|path| path != &dest_dir)
            .collect();
        if srcs.is_empty() {
            return false;
        }
        let count = srcs.len();
        let dest = dest_dir.clone();
        run_job(
            state.clone(),
            ui.clone(),
            format!(
                "Copying {count} item{} to {}",
                plural(count),
                tab_label(&dest)
            ),
            move || {
                // A drop is not a deliberate paste, so never destroy on conflict.
                let items = native_core::resolve_paste(&srcs, &dest, true, native_core::PasteConflict::KeepBoth)?;
                Ok(JobOutcome::undo(
                    format!("Copied {} item{}", items.len(), plural(items.len())),
                    UndoAction::Pasted {
                        items,
                        was_copy: true,
                    },
                ))
            },
            |_, _, _| {},
        );
        true
    });
    widget.add_controller(drop);
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

fn set_preview_text(ui: &Ui, text: &str) {
    ui.preview_media_controls
        .set_media_stream(None::<&gtk::MediaStream>);
    ui.preview_media_picture
        .set_paintable(None::<&gdk::Paintable>);
    let zoom = CURRENT_STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.borrow().preview_zoom)
            .unwrap_or(1.0)
    });
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
    ui.preview_media_controls
        .set_media_stream(None::<&gtk::MediaStream>);
    ui.preview_media_picture
        .set_paintable(None::<&gdk::Paintable>);
    let file = gio::File::for_path(PathBuf::from(path));
    let zoom = CURRENT_STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.borrow().preview_zoom)
            .unwrap_or(1.0)
    });
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
    let file = gio::File::for_path(PathBuf::from(path));
    let media = gtk::MediaFile::for_file(&file);
    let zoom = CURRENT_STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.borrow().preview_zoom)
            .unwrap_or(1.0)
    });
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
}
