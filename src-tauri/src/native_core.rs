use adw::gtk::gio;
use adw::gtk::gio::prelude::FileExt;
use adw::gtk::glib;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const IMG_EXT: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "svg", "ico", "avif", "heic",
];
const AUDIO_EXT: [&str; 13] = [
    "mp3", "flac", "wav", "ogg", "m4a", "aac", "opus", "wma", "aiff", "aif", "alac", "oga", "spx",
];
const VIDEO_EXT: [&str; 12] = [
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "wmv", "flv", "ts", "3gp", "ogv", "mpeg",
];

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
    pub is_image: bool,
    pub icon: String,
    /// A failed metadata lookup must not make a directory entry disappear.
    /// This is especially important for FUSE/network mounts, which can list a
    /// name successfully while a follow-up stat request temporarily fails.
    pub metadata_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DirectoryListing {
    pub entries: Vec<Entry>,
    /// Entries we could name but not stat. They are still included in `entries`.
    pub metadata_unavailable: usize,
    /// Iterator errors have no dependable filename to display, so report their
    /// count instead of silently pretending the directory was complete.
    pub unreadable_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListDirError {
    Cancelled,
    Open(String),
}

impl std::fmt::Display for ListDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "folder loading was cancelled"),
            Self::Open(error) => f.write_str(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedItem {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileProperties {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: i64,
    pub created: i64,
    pub permissions: String,
    pub items: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Drive {
    pub name: String,
    pub path: String,
    pub mountpoint: String,
    pub size: u64,
    pub fstype: String,
    pub kind: String,
    pub removable: bool,
    /// Filesystem total and user-available bytes; `None` when unmounted or unreadable.
    pub space: Option<Space>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Space {
    pub total: u64,
    pub available: u64,
}

/// What a drive row shows: the words, and the bar fill when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceView {
    pub words: String,
    pub fraction: Option<f64>,
}

/// Bytes from statvfs block counts. `bavail` is what a user can use; `bfree`
/// is never consulted. Zero total is no space at all.
pub fn space_from_blocks(frsize: u64, blocks: u64, bavail: u64) -> Option<Space> {
    let total = blocks.saturating_mul(frsize);
    if total == 0 {
        return None;
    }
    Some(Space {
        total,
        available: bavail.saturating_mul(frsize).min(total),
    })
}

pub fn probe_space(mountpoint: &str) -> Option<Space> {
    let path = std::ffi::CString::new(mountpoint).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    space_from_blocks(stat.f_frsize as u64, stat.f_blocks as u64, stat.f_bavail as u64)
}

pub fn space_view(space: Option<Space>) -> SpaceView {
    match space.filter(|s| s.total > 0) {
        Some(s) => {
            let available = s.available.min(s.total);
            SpaceView {
                words: format!("{} available", fmt_size(available)),
                fraction: Some((s.total - available) as f64 / s.total as f64),
            }
        }
        None => SpaceView {
            words: "Space unavailable".to_string(),
            fraction: None,
        },
    }
}

impl Drive {
    pub fn space_view(&self) -> SpaceView {
        if self.mountpoint.is_empty() {
            return space_view(None);
        }
        space_view(self.space)
    }
}

pub fn parent_dir(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
}

pub fn standard_dirs() -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let mut out = vec![("Home".to_string(), home.clone())];
    for dir in [
        "Desktop",
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Videos",
    ] {
        let path = format!("{home}/{dir}");
        if Path::new(&path).is_dir() {
            out.push((dir.to_string(), path));
        }
    }
    out.push(("Root".to_string(), "/".to_string()));
    let trash = format!("{home}/.local/share/Trash/files");
    if Path::new(&trash).is_dir() {
        out.push(("Trash".to_string(), trash));
    }
    out
}

pub fn start_path_from_args() -> Option<String> {
    let arg = std::env::args()
        .nth(1)
        .filter(|arg| !arg.starts_with('-'))?;
    let arg = arg_to_path(&arg);
    let path = Path::new(&arg);
    if path.is_dir() {
        Some(arg)
    } else if path.is_file() {
        path.parent().map(|dir| dir.to_string_lossy().to_string())
    } else {
        None
    }
}

/// The file to highlight when launched pointing at a FILE (e.g. a browser's
/// "show in folder" after a download). `start_path_from_args` only yields the
/// containing folder, so the filename would otherwise be lost.
pub fn select_path_from_args() -> Option<String> {
    let arg = std::env::args()
        .nth(1)
        .filter(|arg| !arg.starts_with('-'))?;
    let arg = arg_to_path(&arg);
    Path::new(&arg).is_file().then_some(arg)
}

fn arg_to_path(arg: &str) -> String {
    if let Some(rest) = arg.strip_prefix("file://") {
        return percent_decode(rest);
    }
    arg.to_string()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn list_dir_cancellable(
    path: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<DirectoryListing, ListDirError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(ListDirError::Cancelled);
    }
    let rd = fs::read_dir(path).map_err(|err| ListDirError::Open(err.to_string()))?;
    let mut out = Vec::new();
    let mut metadata_unavailable = 0;
    let mut unreadable_entries = 0;
    for item in rd {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ListDirError::Cancelled);
        }
        let ent = match item {
            Ok(entry) => entry,
            Err(_) => {
                unreadable_entries += 1;
                continue;
            }
        };
        let name = ent.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let path_buf = ent.path();
        // Use `fs::metadata` rather than `DirEntry::metadata`: the latter can
        // describe the symlink itself, which hides a broken target. We want the
        // same stat operation a file manager needs for size/date/type, while
        // keeping the entry visible when that stat fails.
        let (is_dir, size, modified, metadata_error) = match fs::metadata(&path_buf) {
            Ok(metadata) => (
                metadata.is_dir(),
                metadata.len(),
                metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or(0),
                None,
            ),
            Err(error) => {
                metadata_unavailable += 1;
                (
                    ent.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
                    0,
                    0,
                    Some(error.to_string()),
                )
            }
        };
        out.push(Entry {
            name,
            path: path_buf.to_string_lossy().to_string(),
            is_dir,
            size,
            modified,
            is_image: is_image(&path_buf),
            icon: icon_name(&path_buf, is_dir).to_string(),
            metadata_error,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(DirectoryListing {
        entries: out,
        metadata_unavailable,
        unreadable_entries,
    })
}

pub fn search_dir(
    root: &str,
    query: &str,
    content: bool,
    show_hidden: bool,
) -> Result<Vec<Entry>, String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(vec![]);
    }

    const MAX_RESULTS: usize = 2000;
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::from(root)];

    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_RESULTS {
            break;
        }
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let md = match ent.metadata() {
                Ok(md) => md,
                Err(_) => continue,
            };
            let path = ent.path();
            let is_dir = md.is_dir();
            let is_symlink = ent
                .file_type()
                .map(|kind| kind.is_symlink())
                .unwrap_or(false);
            let mut matched = name.to_lowercase().contains(&query);
            if !matched && content && !is_dir {
                matched = file_contains(&path, &query);
            }
            if matched {
                out.push(Entry {
                    name,
                    path: path.to_string_lossy().to_string(),
                    is_dir,
                    size: md.len(),
                    modified: md
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_secs() as i64)
                        .unwrap_or(0),
                    is_image: is_image(&path),
                    icon: icon_name(&path, is_dir).to_string(),
                    metadata_error: None,
                });
                if out.len() >= MAX_RESULTS {
                    break;
                }
            }
            if is_dir && !is_symlink {
                stack.push(path);
            }
        }
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

pub fn list_drives() -> Vec<Drive> {
    list_drives_with(&probe_space)
}

/// Discovery plus one space probe per mounted drive; blocking, so callers run it off the window thread.
pub fn list_drives_with(probe: &dyn Fn(&str) -> Option<Space>) -> Vec<Drive> {
    let mut out = Vec::new();

    if let Ok(output) = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT,PATH,TYPE,RM",
        ])
        .output()
    {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            fn walk(node: &serde_json::Value, out: &mut Vec<Drive>) {
                let fstype = node["fstype"].as_str().unwrap_or("");
                let kind = node["type"].as_str().unwrap_or("");
                let mountpoint = node["mountpoint"].as_str().unwrap_or("");
                let boring = fstype.is_empty()
                    || fstype == "swap"
                    || mountpoint == "/"
                    || mountpoint == "/boot"
                    || mountpoint == "/boot/efi"
                    || fstype == "crypto_LUKS";

                if matches!(kind, "part" | "disk" | "crypt") && !boring {
                    let label = node["label"].as_str().filter(|value| !value.is_empty());
                    let name = node["name"].as_str().unwrap_or("?");
                    let removable =
                        node["rm"].as_bool().unwrap_or(false) || node["rm"].as_str() == Some("1");
                    out.push(Drive {
                        name: label
                            .map(ToString::to_string)
                            .unwrap_or_else(|| name.to_string()),
                        path: node["path"].as_str().unwrap_or("").to_string(),
                        mountpoint: mountpoint.to_string(),
                        size: node["size"].as_u64().unwrap_or(0),
                        fstype: fstype.to_string(),
                        kind: "drive".to_string(),
                        removable,
                        space: None,
                    });
                }

                if let Some(children) = node["children"].as_array() {
                    for child in children {
                        walk(child, out);
                    }
                }
            }

            if let Some(devices) = value["blockdevices"].as_array() {
                for device in devices {
                    walk(device, &mut out);
                }
            }
        }
    }

    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 {
                continue;
            }
            let (device, mountpoint, fstype) = (fields[0], fields[1], fields[2]);
            if mountpoint.contains("/GoogleDrive") || fstype.contains("rclone") {
                out.push(Drive {
                    name: "Google Drive".to_string(),
                    path: mountpoint.to_string(),
                    mountpoint: mountpoint.to_string(),
                    size: 0,
                    fstype: "rclone".to_string(),
                    kind: "gdrive".to_string(),
                    removable: false,
                    space: None,
                });
            } else if (fstype.starts_with("nfs") || fstype == "cifs" || fstype.starts_with("fuse."))
                && mountpoint.starts_with("/home")
                && !mountpoint.contains("/doc")
            {
                out.push(Drive {
                    name: mountpoint.rsplit('/').next().unwrap_or(device).to_string(),
                    path: mountpoint.to_string(),
                    mountpoint: mountpoint.to_string(),
                    size: 0,
                    fstype: fstype.to_string(),
                    kind: "network".to_string(),
                    removable: false,
                    space: None,
                });
            }
        }
    }

    for drive in out.iter_mut().filter(|d| !d.mountpoint.is_empty()) {
        drive.space = probe(&drive.mountpoint);
    }

    out
}

/// Result of running the mount command: `Ok(())` on a zero exit, `Err(stderr)` on
/// a nonzero one. The outer `Result` is a failure to run it at all.
pub type MountRun = Result<Result<(), String>, String>;

pub fn mount_drive(device: &str) -> Result<Mounted, String> {
    mount_drive_with(
        device,
        &|device| {
            let output = Command::new("udisksctl")
                .args(["mount", "-b", device])
                .output()
                .map_err(|err| err.to_string())?;
            Ok(if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
            })
        },
        &|device| {
            let output = Command::new("lsblk")
                .args(["-J", "-o", "PATH,MOUNTPOINTS", device])
                .output()
                .map_err(|err| err.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        },
    )
}

/// How a drive came to be mounted at the verified location.
#[derive(Debug, PartialEq, Eq)]
pub enum Mounted {
    /// The mount command exited zero and system state confirms the location.
    New(String),
    /// The mount command failed, but system state proves this exact device is
    /// already mounted at the location (another component mounted it first).
    Already(String),
}

/// A zero exit only means the command finished. The location comes from system
/// state (`lookup` returns structured `lsblk -J` output), never from stdout.
/// A nonzero exit is recovered only when that same state proves the device is
/// already mounted at one verified directory; the command's wording is never
/// inspected, and any other failure keeps its original error.
pub fn mount_drive_with(
    device: &str,
    run: &dyn Fn(&str) -> MountRun,
    lookup: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Mounted, String> {
    match run(device)? {
        Ok(()) => {
            let none = "Mount command completed but no usable mountpoint was found";
            let json = lookup(device).map_err(|err| format!("{none}: {err}"))?;
            verified_mountpoint(device, &json)
                .map(Mounted::New)
                .map_err(|err| format!("{none}: {err}"))
        }
        Err(original) => lookup(device)
            .ok()
            .and_then(|json| verified_mountpoint(device, &json).ok())
            .map(Mounted::Already)
            .ok_or(original),
    }
}

/// The one directory `device` is mounted at, or an error. Fail-closed on parse
/// errors, no mountpoint, several distinct mountpoints, and any path that is not
/// absolute or not an existing directory.
pub fn verified_mountpoint(device: &str, lsblk_json: &str) -> Result<String, String> {
    fn find<'a>(node: &'a serde_json::Value, device: &str, out: &mut Vec<&'a serde_json::Value>) {
        if node["path"].as_str() == Some(device) {
            out.push(node);
        }
        if let Some(children) = node["children"].as_array() {
            for child in children {
                find(child, device, out);
            }
        }
    }
    let value: serde_json::Value =
        serde_json::from_str(lsblk_json).map_err(|err| format!("unreadable lsblk output: {err}"))?;
    let mut nodes = Vec::new();
    for node in value["blockdevices"].as_array().ok_or("unreadable lsblk output")? {
        find(node, device, &mut nodes);
    }
    if nodes.len() != 1 {
        return Err(format!("{device} not found exactly once in lsblk output"));
    }
    let node = nodes[0];
    let mut candidates: Vec<String> = Vec::new();
    let mut add = |v: &serde_json::Value| {
        if let Some(text) = v.as_str().filter(|t| !t.is_empty()) {
            if !candidates.iter().any(|c| c == text) {
                candidates.push(text.to_string());
            }
        }
    };
    add(&node["mountpoint"]);
    if let Some(list) = node["mountpoints"].as_array() {
        list.iter().for_each(&mut add);
    }
    match candidates.len() {
        0 => Err(format!("{device} has no mountpoint")),
        1 => {
            let path = candidates.remove(0);
            if !Path::new(&path).is_absolute() {
                return Err(format!("mountpoint is not absolute: {path}"));
            }
            if !fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                return Err(format!("mountpoint is not a directory: {path}"));
            }
            Ok(path)
        }
        _ => Err(format!("{device} has several mountpoints: {}", candidates.join(", "))),
    }
}

pub fn unmount_drive(device: &str) -> Result<(), String> {
    run_cmd_capture("udisksctl", &["unmount", "-b", device])
}

pub fn eject_drive(device: &str) -> Result<(), String> {
    let disk = whole_disk_of(device);
    if let Ok(output) = Command::new("lsblk")
        .args(["-nro", "PATH,MOUNTPOINT", &disk])
        .output()
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut parts = line.split_whitespace();
            let part = parts.next().unwrap_or("");
            let mountpoint = parts.next().unwrap_or("");
            if !part.is_empty() && !mountpoint.is_empty() {
                let _ = Command::new("udisksctl")
                    .args(["unmount", "-b", part])
                    .output();
            }
        }
    }
    run_cmd_capture("udisksctl", &["power-off", "-b", &disk])
}

/// Starts an external program without waiting for it to finish and reaps it
/// on its own thread, so an exited child never lingers as a zombie while
/// Wayfinder keeps running. Returns the child's PID as soon as it has started.
/// Launch errors are returned unchanged; if the reaper thread cannot be
/// started the child is killed and reaped here and that failure is returned.
fn spawn_reaped(mut command: Command) -> std::io::Result<u32> {
    let program = command.get_program().to_string_lossy().into_owned();
    let child = command.spawn()?;
    let pid = child.id();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let for_thread = std::sync::Arc::clone(&slot);
    let thread_program = program.clone();
    let started = std::thread::Builder::new()
        .name(format!("wf-reap-{pid}"))
        .spawn(move || {
            let child = for_thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(mut child) = child {
                if let Err(err) = child.wait() {
                    eprintln!("wayfinder: waiting for {thread_program} (pid {pid}) failed: {err}");
                }
            }
        });
    match started {
        Ok(_) => Ok(pid),
        Err(err) => {
            let child = slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            let mut cleanup = String::new();
            if let Some(mut child) = child {
                if let Err(kill_err) = child.kill() {
                    cleanup.push_str(&format!("; kill failed: {kill_err}"));
                }
                if let Err(wait_err) = child.wait() {
                    cleanup.push_str(&format!("; child {pid} was not confirmed reaped: {wait_err}"));
                }
            }
            Err(std::io::Error::other(format!(
                "could not start a reaper for {program}: {err}{cleanup}"
            )))
        }
    }
}

pub fn open_path(path: &str) -> Result<(), String> {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    spawn_reaped(command)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub fn open_with_apps(path: &str) -> Vec<AppEntry> {
    let mime = Command::new("xdg-mime")
        .args(["query", "filetype", path])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    if mime.is_empty() {
        return vec![];
    }

    let dirs = app_dirs();
    let mut ids: Vec<String> = Vec::new();
    for dir in &dirs {
        if let Ok(text) = fs::read_to_string(format!("{dir}/mimeinfo.cache")) {
            for line in text.lines() {
                if let Some((line_mime, apps)) = line.split_once('=') {
                    if line_mime == mime {
                        for app in apps.split(';') {
                            if !app.is_empty() && !ids.iter().any(|seen| seen == app) {
                                ids.push(app.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for id in ids.into_iter().take(15) {
        let mut name = id.trim_end_matches(".desktop").to_string();
        let mut hidden = false;
        for dir in &dirs {
            if let Ok(text) = fs::read_to_string(format!("{dir}/{id}")) {
                for line in text.lines() {
                    if let Some(value) = line.strip_prefix("Name=") {
                        name = value.to_string();
                    }
                    if matches!(line, "NoDisplay=true" | "Terminal=true" | "Hidden=true") {
                        hidden = true;
                    }
                }
                break;
            }
        }
        if !hidden {
            out.push(AppEntry { id, name });
        }
    }
    out
}

/// The desktop file that `app_id` names, searched in the same order the Open
/// With list is built from.
pub fn open_with_desktop_file(app_id: &str) -> Result<String, String> {
    for dir in app_dirs() {
        let desktop_file = format!("{dir}/{app_id}");
        if Path::new(&desktop_file).exists() {
            return Ok(desktop_file);
        }
    }
    Err("app not found".into())
}

/// One terminal to try: the program that must exist, and the full command line
/// that opens it in the wanted directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCandidate {
    pub program: &'static str,
    pub argv: Vec<String>,
}

/// Konsole first, then the rest, in the order they are tried. Terminals with no
/// working-directory flag are started through `sh`, which changes directory
/// first, so the directory is exact for every candidate.
pub fn terminal_candidates(path: &str) -> Vec<TerminalCandidate> {
    let with_flag = |program: &'static str, flag: &str| TerminalCandidate {
        program,
        argv: vec![program.to_string(), flag.to_string(), path.to_string()],
    };
    let via_sh = |program: &'static str| TerminalCandidate {
        program,
        argv: vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("cd \"$1\" && exec {program}"),
            "sh".to_string(),
            path.to_string(),
        ],
    };
    vec![
        with_flag("konsole", "--workdir"),
        with_flag("kgx", "--working-directory"),
        with_flag("gnome-terminal", "--working-directory"),
        via_sh("x-terminal-emulator"),
        via_sh("xterm"),
    ]
}

/// What one attempt to start a child application came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The program is not installed: try the next candidate.
    NotFound,
    /// It exists but could not be started: report once and stop.
    Failed(String),
}

/// Walks the terminal candidates in order. A missing program falls through to
/// the next; any other failure is returned at once, and only once.
pub fn open_terminal_with(
    path: &str,
    mut launch: impl FnMut(&TerminalCandidate) -> Result<(), LaunchError>,
) -> Result<(), String> {
    for candidate in terminal_candidates(path) {
        match launch(&candidate) {
            Ok(()) => return Ok(()),
            Err(LaunchError::NotFound) => continue,
            Err(LaunchError::Failed(err)) => return Err(err),
        }
    }
    Err("no supported terminal found".into())
}

pub fn read_text_preview(path: &str) -> Result<String, String> {
    use std::io::Read;

    let md = fs::metadata(path).map_err(|err| err.to_string())?;
    if !md.is_file() {
        return Err("not a file".into());
    }

    const CAP: usize = 256 * 1024;
    let mut file = fs::File::open(path).map_err(|err| err.to_string())?;
    let mut buf = vec![0u8; CAP];
    let mut n = 0;
    while n < CAP {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(err) => return Err(err.to_string()),
        }
    }
    buf.truncate(n);
    if buf.contains(&0) {
        return Err("binary".into());
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if md.len() as usize > n {
        text.push_str("\n\n... (preview truncated, showing the first 256 KB)");
    }
    Ok(text)
}

/// Explorer numbering: "New Folder", then "New Folder (2)", "New Folder (3)"...
/// `create_dir` is the existence check, so a sibling that appears between two
/// attempts is skipped rather than overwritten or raced.
pub fn make_numbered_dir(parent: &str, base: &str) -> Result<String, String> {
    validate_name(base)?;
    for n in 1..=100_000u32 {
        let name = if n == 1 { base.to_string() } else { format!("{base} ({n})") };
        let path = Path::new(parent).join(&name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path.to_string_lossy().to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("no free folder name".into())
}

pub fn new_file(parent: &str, name: &str) -> Result<String, String> {
    validate_name(name)?;
    let path = Path::new(parent).join(name);
    if path.exists() {
        return Err("already exists".into());
    }
    fs::File::create(&path).map_err(|err| err.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

pub fn rename_path(path: &str, new_name: &str) -> Result<String, String> {
    validate_name(new_name)?;
    let parent = Path::new(path).parent().ok_or("no parent")?;
    let dest = parent.join(new_name);
    if dest.as_path() == Path::new(path) {
        return Ok(dest.to_string_lossy().to_string());
    }
    // fs::rename silently replaces an existing file on Linux; never do that.
    if dest.symlink_metadata().is_ok() {
        return Err(format!("\"{new_name}\" already exists"));
    }
    fs::rename(path, &dest).map_err(|err| err.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

pub fn batch_rename(items: &[(String, String)]) -> Result<Vec<ResolvedItem>, String> {
    batch_rename_with(items, &|_| Ok(()))
}

/// All-or-nothing batch rename: the request is planned and checked without
/// mutation, every source is staged to an owned same-parent temporary, then every
/// temporary is placed at its destination. Any error reverses the placements and
/// restores every staged source to its exact original path; an `Err` therefore
/// leaves no visible rename and no owned temporary, or names each failed
/// restoration. Not crash-atomic. `step` is a test seam called before each
/// "stage" and each "place".
pub fn batch_rename_with(
    items: &[(String, String)],
    step: &dyn Fn(&str) -> Result<(), String>,
) -> Result<Vec<ResolvedItem>, String> {
    let mut operations = Vec::new();
    let mut destinations = HashSet::new();
    let mut moving_from = HashSet::new();

    for (path, new_name) in items {
        validate_name(new_name)?;
        let from = PathBuf::from(path);
        let parent = from.parent().ok_or("no parent")?.to_path_buf();
        let dest = parent.join(new_name);
        if from == dest {
            continue;
        }
        if !moving_from.insert(from.to_string_lossy().to_string()) {
            return Err(format!("duplicate source: {path}"));
        }
        let dest_key = dest.to_string_lossy().to_string();
        if !destinations.insert(dest_key) {
            return Err(format!("duplicate target name: {new_name}"));
        }
        operations.push((from, dest));
    }

    if operations.is_empty() {
        return Ok(vec![]);
    }

    for (from, dest) in &operations {
        if !entry_exists(from)? {
            return Err(format!("missing source: {}", from.to_string_lossy()));
        }
        if entry_exists(dest)? && !moving_from.contains(&dest.to_string_lossy().to_string()) {
            return Err(format!("target already exists: {}", dest.to_string_lossy()));
        }
    }

    let pid = std::process::id();
    let mut staged: Vec<PathBuf> = Vec::new();
    let mut placed = 0usize;
    let result = (|| -> Result<(), String> {
        for (index, (from, _)) in operations.iter().enumerate() {
            step("stage")?;
            let parent = from.parent().ok_or("no parent")?;
            let mut temp = parent.join(format!(".wayfinder-rename-{pid}-{index}.tmp"));
            let mut suffix = 0;
            while entry_exists(&temp)? {
                suffix += 1;
                temp = parent.join(format!(".wayfinder-rename-{pid}-{index}-{suffix}.tmp"));
            }
            fs::rename(from, &temp).map_err(|err| err.to_string())?;
            staged.push(temp);
        }
        for (index, (_, dest)) in operations.iter().enumerate() {
            step("place")?;
            fs::rename(&staged[index], dest).map_err(|err| err.to_string())?;
            placed = index + 1;
        }
        Ok(())
    })();

    if let Err(err) = result {
        let mut failures = Vec::new();
        // Barrier 1: vacate every completed destination, newest first, restoring no original.
        let mut still_placed = HashSet::new();
        for index in (0..placed).rev() {
            if let Err(e) = fs::rename(&operations[index].1, &staged[index]) {
                failures.push(format!("{}: {e}", operations[index].1.display()));
                still_placed.insert(index);
            }
        }
        // Barrier 2: only now return each temporary to its original path, never
        // onto an original that a placed item may still occupy.
        for index in (0..staged.len()).rev() {
            if still_placed.contains(&index) {
                continue;
            }
            let original = &operations[index].0;
            if still_placed.iter().any(|&i| operations[i].1 == *original) {
                failures.push(format!(
                    "{}: not restored, {} still occupied",
                    staged[index].display(),
                    original.display()
                ));
                continue;
            }
            if let Err(e) = fs::rename(&staged[index], original) {
                failures.push(format!("{}: {e}", staged[index].display()));
            }
        }
        return Err(if failures.is_empty() {
            err
        } else {
            format!("{err}; rollback incomplete: {}", failures.join("; "))
        });
    }

    Ok(operations
        .into_iter()
        .map(|(from, dest)| ResolvedItem {
            from: from.to_string_lossy().to_string(),
            to: dest.to_string_lossy().to_string(),
        })
        .collect())
}

/// Build an RFC 2483 `text/uri-list` payload (CRLF-separated `file://` URIs) for
/// a drag-out to browsers / Electron apps that don't understand GdkFileList.
pub fn uri_list_for_paths(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Move paths to the Trash.
///
/// Uses GIO's trash API directly. This used to shell out to `gio trash` once per
/// path and block on each child, so batch-deleting N files meant N sequential
/// process spawns on the UI thread — which froze the window.
pub fn trash_paths(paths: &[String]) -> Result<(), String> {
    for path in paths {
        gio::File::for_path(path)
            .trash(gio::Cancellable::NONE)
            .map_err(|err| format!("{path}: {err}"))?;
    }
    Ok(())
}

pub fn restore_from_trash(paths: &[String]) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let trash = Path::new(&home).join(".local/share/Trash");
    for original in paths {
        restore_from_trash_in(&trash, original)?;
    }
    Ok(())
}

struct TrashEntry {
    info: PathBuf,
    payload: PathBuf,
    destination: PathBuf,
}

fn parse_trashinfo(content: &str) -> Option<(String, String)> {
    let mut original = None;
    let mut date = String::new();
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("Path=") {
            original = Some(percent_decode(value.trim()));
        } else if let Some(value) = line.strip_prefix("DeletionDate=") {
            date = value.trim().to_string();
        }
    }
    original.map(|path| (path, date))
}

/// Resolves the selector (an original path, or a direct child of this Trash's
/// `files/` directory) to the on-disk payload, its `.trashinfo` and the exact
/// decoded destination. Everything resolved stays inside `trash`.
fn resolve_trash_entry(trash: &Path, selector: &str) -> Result<TrashEntry, String> {
    let info_dir = trash.join("info");
    let files_dir = trash.join("files");
    let selected = Path::new(selector);
    let explicit = match (selected.parent(), selected.file_name()) {
        (Some(parent), Some(name)) if parent == files_dir => name.to_str().map(str::to_string),
        _ => None,
    };
    let stem = match explicit {
        Some(stem) => stem,
        None => {
            let mut best: Option<(String, String)> = None;
            if let Ok(entries) = fs::read_dir(&info_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("trashinfo") {
                        continue;
                    }
                    let Some((original, date)) = fs::read_to_string(&path)
                        .ok()
                        .and_then(|content| parse_trashinfo(&content))
                    else {
                        continue;
                    };
                    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                        continue;
                    };
                    if original == selector
                        && best.as_ref().map(|(_, d)| date > *d).unwrap_or(true)
                    {
                        best = Some((stem.to_string(), date));
                    }
                }
            }
            match best {
                Some((stem, _)) => stem,
                None => return Err(format!("not found in trash: {selector}")),
            }
        }
    };
    let info = info_dir.join(format!("{stem}.trashinfo"));
    match fs::symlink_metadata(&info) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => {
            return Err(format!(
                "trash metadata is not a regular file: {}",
                info.display()
            ))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("not found in trash: {selector}"))
        }
        Err(err) => return Err(format!("cannot read trash metadata {}: {err}", info.display())),
    }
    let content = fs::read_to_string(&info)
        .map_err(|err| format!("cannot read trash metadata {}: {err}", info.display()))?;
    let Some((original, _)) = parse_trashinfo(&content) else {
        return Err(format!("malformed trash metadata: {}", info.display()));
    };
    if !original.starts_with('/') {
        return Err(format!(
            "malformed trash metadata (Path is not absolute): {}",
            info.display()
        ));
    }
    let payload = files_dir.join(&stem);
    if fs::symlink_metadata(&payload).is_err() {
        return Err(format!("trash payload missing: {}", payload.display()));
    }
    Ok(TrashEntry {
        info,
        payload,
        destination: PathBuf::from(original),
    })
}

/// Moves `from` to `to` without ever replacing an existing file, directory or
/// dangling symlink at `to` (one atomic kernel call, no exists-then-rename race).
fn rename_no_replace(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let c = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("path contains a NUL byte: {}", path.display()))
    };
    let (from_c, to_c) = (c(from)?, c(to)?);
    // SAFETY: both C strings are NUL-terminated and stay alive for the whole call.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EEXIST) {
        Err(format!("already exists: {}", to.display()))
    } else {
        Err(format!("{}: {err}", to.display()))
    }
}

fn restore_from_trash_in(trash: &Path, selector: &str) -> Result<(), String> {
    restore_with(trash, selector, &|info| fs::remove_file(info))
}

/// The one restore implementation; `remove_info` is injectable only so tests can
/// force the metadata-removal failure that triggers the rollback.
fn restore_with(
    trash: &Path,
    selector: &str,
    remove_info: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let entry = resolve_trash_entry(trash, selector)?;
    let parent = entry
        .destination
        .parent()
        .ok_or_else(|| format!("no destination folder: {}", entry.destination.display()))?;
    if !parent.is_dir() {
        return Err(format!("destination folder is missing: {}", parent.display()));
    }
    rename_no_replace(&entry.payload, &entry.destination)
        .map_err(|err| format!("cannot restore from trash: {err}"))?;
    if let Err(err) = remove_info(&entry.info) {
        return match rename_no_replace(&entry.destination, &entry.payload) {
            Ok(()) => Err(format!(
                "could not remove {}: {err}; item left in trash at {}",
                entry.info.display(),
                entry.payload.display()
            )),
            Err(back) => Err(format!(
                "could not remove {}: {err}; could not move back ({back}); the item is at {} and the trash path {} is occupied",
                entry.info.display(),
                entry.destination.display(),
                entry.payload.display()
            )),
        };
    }
    Ok(())
}

pub fn delete_permanent(paths: &[String]) -> Result<(), String> {
    for path in paths {
        let path = Path::new(path);
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|err| err.to_string())?;
        } else {
            fs::remove_file(path).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

/// Names that would clash at the destination. A MOVE onto its own path is a
/// no-op, not a clash; a COPY onto its own path clashes (Duplicate is offered).
pub fn check_conflicts(srcs: &[String], dest_dir: &str, is_copy: bool) -> Vec<String> {
    srcs.iter()
        .filter_map(|src| {
            let name = Path::new(src).file_name()?.to_str()?.to_string();
            let target = Path::new(dest_dir).join(&name);
            if !target.exists() {
                return None;
            }
            let same_path = matches!(
                (fs::canonicalize(src), fs::canonicalize(&target)),
                (Ok(a), Ok(b)) if a == b
            );
            (is_copy || !same_path).then_some(name)
        })
        .collect()
}

/// True when this is a MOVE and every item is already at its destination path:
/// nothing to do, so no job, undo record or clipboard change should follow.
pub fn is_noop_move(srcs: &[String], dest_dir: &str, is_copy: bool) -> bool {
    !is_copy
        && !srcs.is_empty()
        && srcs.iter().all(|src| {
            let Some(name) = Path::new(src).file_name() else {
                return false;
            };
            matches!(
                (fs::canonicalize(src), fs::canonicalize(Path::new(dest_dir).join(name))),
                (Ok(a), Ok(b)) if a == b
            )
        })
}

/// Maps a clash-dialog response to what to do. Cancel (and Escape / close)
/// is None: no transfer may start.
pub fn conflict_response(response: &str) -> Option<PasteConflict> {
    match response {
        "replace" => Some(PasteConflict::Replace),
        "duplicate" => Some(PasteConflict::KeepBoth),
        _ => None,
    }
}

/// What to do when the destination already holds a file of the same name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PasteConflict {
    /// Paste alongside, as "name (2)". Never destroys anything.
    KeepBoth,
    /// Overwrite the existing item. DESTRUCTIVE and not undoable — the undo
    /// stack can move the pasted item back out, but whatever was overwritten is
    /// already gone.
    Replace,
}

/// Explorer drag action: Ctrl copies, Shift moves, otherwise MOVE only when
/// every source and the destination folder are on the same device. If any
/// metadata cannot be read the choice is an explicit conservative COPY.
pub fn drag_is_copy(srcs: &[String], dest_dir: &str, ctrl: bool, shift: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    if ctrl {
        return true;
    }
    if shift {
        return false;
    }
    let Ok(dest) = fs::metadata(dest_dir) else {
        return true;
    };
    !srcs.iter().all(|src| {
        fs::symlink_metadata(src)
            .map(|m| m.dev() == dest.dev())
            .unwrap_or(false)
    })
}

/// What a source looked like before a transfer: identity plus a content
/// fingerprint, so the result can be checked against the disk afterwards.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceSnapshot {
    pub path: String,
    pub ino: u64,
    pub fp: Fingerprint,
}

/// Exact content fingerprints (64-bit SipHash over every byte, streamed with
/// bounded memory at any size). Files: length and digest of all bytes. Folders:
/// a digest over the sorted relative names, entry types, symlink targets and
/// every regular-file byte, so renamed or edited entries differ. Symlinks: the
/// link target. Unreadable entries or a read that ends early are errors.
#[derive(Clone, Debug, PartialEq)]
pub enum Fingerprint {
    File { len: u64, digest: u64 },
    Tree { entries: u64, digest: u64 },
    Link(String),
}

fn hash_file_bytes(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    path: &Path,
    len: u64,
) -> Result<(), String> {
    use std::hash::Hasher;
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut seen = 0u64;
    loop {
        let got = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if got == 0 {
            break;
        }
        hasher.write(&buf[..got]);
        seen += got as u64;
    }
    if seen != len {
        return Err(format!("{}: read {seen} of {len} bytes", path.display()));
    }
    Ok(())
}

fn hash_tree(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    dir: &Path,
    entries: &mut u64,
) -> Result<(), String> {
    use std::hash::Hasher;
    let mut names = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.file_name()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        *entries += 1;
        hasher.write(name.to_string_lossy().as_bytes());
        hasher.write_u8(0);
        if meta.file_type().is_symlink() {
            hasher.write_u8(b'L');
            let target = fs::read_link(&path).map_err(|e| e.to_string())?;
            hasher.write(target.to_string_lossy().as_bytes());
        } else if meta.is_dir() {
            hasher.write_u8(b'D');
            hash_tree(hasher, &path, entries)?;
            hasher.write_u8(b'/');
        } else {
            hasher.write_u8(b'F');
            hasher.write_u64(meta.len());
            hash_file_bytes(hasher, &path, meta.len())?;
        }
    }
    Ok(())
}

pub fn fingerprint(path: &str) -> Result<Fingerprint, String> {
    use std::hash::Hasher;
    let p = Path::new(path);
    let meta = fs::symlink_metadata(p).map_err(|e| format!("{path}: {e}"))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if meta.file_type().is_symlink() {
        let target = fs::read_link(p).map_err(|e| e.to_string())?;
        Ok(Fingerprint::Link(target.to_string_lossy().to_string()))
    } else if meta.is_dir() {
        let mut entries = 0;
        hash_tree(&mut hasher, p, &mut entries)?;
        Ok(Fingerprint::Tree { entries, digest: hasher.finish() })
    } else {
        hash_file_bytes(&mut hasher, p, meta.len())?;
        Ok(Fingerprint::File { len: meta.len(), digest: hasher.finish() })
    }
}

/// Fails if any requested source cannot be recorded.
pub fn snapshot_sources(srcs: &[String]) -> Result<Vec<SourceSnapshot>, String> {
    use std::os::unix::fs::MetadataExt;
    srcs.iter()
        .map(|p| {
            let meta = fs::symlink_metadata(p).map_err(|e| format!("cannot read {p}: {e}"))?;
            Ok(SourceSnapshot { path: p.clone(), ino: meta.ino(), fp: fingerprint(p)? })
        })
        .collect()
}

/// Fail-closed postcondition. COPY: every source still present with the same
/// identity and content, every destination present with matching content. MOVE:
/// every moved source gone and every destination present with matching content.
pub fn verify_transfer(
    before: &[SourceSnapshot],
    items: &[ResolvedItem],
    is_copy: bool,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    for item in items {
        let Some(src) = before.iter().find(|s| s.path == item.from) else {
            return Err(format!("transfer check: no record of {}", item.from));
        };
        let dest = fingerprint(&item.to).map_err(|_| {
            format!("transfer check: {} is missing after the transfer", item.to)
        })?;
        if dest != src.fp {
            return Err(format!("transfer check: {} does not match its source", item.to));
        }
        let now = fs::symlink_metadata(&item.from);
        if is_copy {
            let now = now
                .map_err(|_| format!("transfer check: copy removed the source {}", item.from))?;
            if now.ino() != src.ino || fingerprint(&item.from)? != src.fp {
                return Err(format!("transfer check: copy changed the source {}", item.from));
            }
        } else if now.is_ok() {
            return Err(format!("transfer check: move left the source {}", item.from));
        }
    }
    Ok(())
}

/// Result of a committed transfer. `warnings` are cleanup problems after a valid,
/// verified commit: the visible end state is exact and Undo applies.
#[derive(Debug)]
pub struct TransferResult {
    pub items: Vec<ResolvedItem>,
    pub warnings: Vec<String>,
}

/// One live change that can be reversed, in commit order.
enum JournalOp {
    Backup { target: PathBuf, backup: PathBuf },
    Placed { dest: PathBuf },
    Moved { src: PathBuf, dest: PathBuf },
    Hold { src: PathBuf, hold: PathBuf },
}

enum PlanKind {
    Fresh,
    Replace,
}

struct PlanItem {
    src: String,
    dest: PathBuf,
    kind: PlanKind,
    /// A staged copy is used (COPY, or a cross-device MOVE).
    stage: Option<PathBuf>,
}

#[cfg(test)]
pub fn resolve_paste(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
) -> Result<Vec<ResolvedItem>, String> {
    resolve_paste_with(srcs, dest_dir, is_copy, on_conflict, &|_| Ok(()))
}

#[cfg(test)]
pub fn resolve_paste_with(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
    step: &dyn Fn(&str) -> Result<(), String>,
) -> Result<Vec<ResolvedItem>, String> {
    transfer_batch(srcs, dest_dir, is_copy, on_conflict, step, false).map(|r| r.items)
}

/// A unique, not-yet-existing sibling name in `dir` for internal staging or backup.
fn internal_sibling(dir: &Path, kind: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = dir.join(format!(".wf-replace-{kind}-{}-{n}", std::process::id()));
        if fs::symlink_metadata(&path).is_err() {
            return path;
        }
    }
}

/// True when a directory entry exists at `path`, without following symlinks (a
/// broken link is an entry). Only `NotFound` means absent; any other error is real.
fn entry_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}

fn remove_any(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path).map_err(|e| e.to_string()),
        Ok(_) => fs::remove_file(path).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}

/// Reverses the journal newest-first and removes owned stages. Every reversal is
/// attempted; failures are returned so the caller reports them explicitly.
fn roll_back(journal: &[JournalOp], stages: &[PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    for op in journal.iter().rev() {
        let result = match op {
            JournalOp::Placed { dest } => remove_any(dest),
            JournalOp::Moved { src, dest } => {
                fs::rename(dest, src).map_err(|e| format!("{}: {e}", dest.display()))
            }
            JournalOp::Hold { src, hold } => {
                fs::rename(hold, src).map_err(|e| format!("{}: {e}", hold.display()))
            }
            JournalOp::Backup { target, backup } => {
                fs::rename(backup, target).map_err(|e| format!("{}: {e}", backup.display()))
            }
        };
        if let Err(err) = result {
            failures.push(err);
        }
    }
    for stage in stages {
        if let Err(err) = remove_any(stage) {
            failures.push(format!("{}: {err}", stage.display()));
        }
    }
    failures
}

/// Transfers a whole batch as one transaction. The batch is planned and every
/// incoming object is prepared and fingerprint-checked before any live path
/// changes. Old destinations and moved sources are then kept in reversible
/// same-filesystem places (backup beside the target, hold beside the source)
/// until the postcondition passes. An `Err` means every source and destination
/// is exactly as before and nothing owned remains (or the error names what could
/// not be rolled back). An `Ok` is committed; cleanup problems become warnings.
/// Not crash-atomic and not safe against concurrent outside changes.
/// `step` is a test seam called at "prepare", "backup", "hold", "place",
/// "verify" and "cleanup"; `force_cross_device` makes MOVE use the cross-device
/// strategy (staged copy plus source hold) even on one filesystem.
pub fn transfer_batch(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
    step: &dyn Fn(&str) -> Result<(), String>,
    force_cross_device: bool,
) -> Result<TransferResult, String> {
    use std::os::unix::fs::MetadataExt;
    if !Path::new(dest_dir).is_dir() {
        return Err(format!("not a folder: {dest_dir}"));
    }
    let dest_dev = fs::metadata(dest_dir).map_err(|e| e.to_string())?.dev();
    let before = snapshot_sources(srcs)?;

    // Plan: no live mutation.
    let mut plan: Vec<PlanItem> = Vec::new();
    for src in srcs {
        let name = Path::new(src)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("bad source name")?;
        let target = Path::new(dest_dir).join(name);
        let same_path = matches!(
            (fs::canonicalize(src), fs::canonicalize(&target)),
            (Ok(a), Ok(b)) if a == b
        );
        if same_path && !is_copy {
            continue;
        }
        let planned = |p: &Path| plan.iter().any(|item| item.dest == p);
        let (dest, kind) = if entry_exists(&target)? || planned(&target) {
            match on_conflict {
                PasteConflict::KeepBoth => {
                    let (stem, ext) = match name.rfind('.') {
                        Some(i) if i > 0 => (&name[..i], &name[i..]),
                        _ => (name, ""),
                    };
                    let mut n = 1;
                    let candidate = loop {
                        let cand = if n == 1 {
                            Path::new(dest_dir).join(name)
                        } else {
                            Path::new(dest_dir).join(format!("{stem} ({n}){ext}"))
                        };
                        n += 1;
                        if !entry_exists(&cand)? && !planned(&cand) {
                            break cand;
                        }
                    };
                    (candidate, PlanKind::Fresh)
                }
                PasteConflict::Replace => {
                    if same_path {
                        continue;
                    }
                    if planned(&target) {
                        return Err(format!("two items would replace the same name: {name}"));
                    }
                    (target, PlanKind::Replace)
                }
            }
        } else {
            (target, PlanKind::Fresh)
        };
        plan.push(PlanItem { src: src.clone(), dest, kind, stage: None });
    }

    let cross = |item: &PlanItem| -> Result<bool, String> {
        Ok(force_cross_device
            || fs::symlink_metadata(&item.src).map_err(|e| e.to_string())?.dev() != dest_dev)
    };

    let mut stages: Vec<PathBuf> = Vec::new();
    let mut journal: Vec<JournalOp> = Vec::new();
    let mut retire: Vec<PathBuf> = Vec::new();
    let mut run = || -> Result<(), String> {
        // Prepare every incoming object beside its destination.
        for item in plan.iter_mut() {
            let src_fp = &before
                .iter()
                .find(|s| s.path == item.src)
                .ok_or("transfer check: no record of a source")?
                .fp;
            if is_copy || cross(item)? {
                let dir = item.dest.parent().ok_or("bad target")?;
                let stage = internal_sibling(dir, "new");
                stages.push(stage.clone());
                let stage_str = stage.to_string_lossy().to_string();
                run_cmd_capture("cp", &["-a", "-T", "--", &item.src, &stage_str])?;
                if fingerprint(&stage_str)? != *src_fp {
                    return Err("the prepared copy does not match its source".to_string());
                }
                item.stage = Some(stage);
            }
            step("prepare")?;
        }
        // Commit in order, journaling every live change.
        for item in plan.iter() {
            if matches!(item.kind, PlanKind::Replace) {
                step("backup")?;
                let dir = item.dest.parent().ok_or("bad target")?;
                let backup = internal_sibling(dir, "old");
                fs::rename(&item.dest, &backup).map_err(|e| e.to_string())?;
                journal.push(JournalOp::Backup { target: item.dest.clone(), backup: backup.clone() });
                retire.push(backup);
            }
            match (&item.stage, is_copy) {
                (Some(stage), true) => {
                    step("place")?;
                    fs::rename(stage, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Placed { dest: item.dest.clone() });
                }
                (Some(stage), false) => {
                    step("hold")?;
                    let src = Path::new(&item.src);
                    let hold = internal_sibling(src.parent().ok_or("bad source")?, "hold");
                    fs::rename(src, &hold).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Hold { src: src.to_path_buf(), hold: hold.clone() });
                    retire.push(hold);
                    step("place")?;
                    fs::rename(stage, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Placed { dest: item.dest.clone() });
                }
                (None, _) => {
                    step("place")?;
                    fs::rename(&item.src, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Moved {
                        src: PathBuf::from(&item.src),
                        dest: item.dest.clone(),
                    });
                }
            }
        }
        step("verify")?;
        let items: Vec<ResolvedItem> = plan
            .iter()
            .map(|i| ResolvedItem { from: i.src.clone(), to: i.dest.to_string_lossy().to_string() })
            .collect();
        verify_transfer(&before, &items, is_copy)
    };
    if let Err(err) = run() {
        let failures = roll_back(&journal, &stages);
        return Err(if failures.is_empty() {
            err
        } else {
            format!("{err}; rollback incomplete: {}", failures.join("; "))
        });
    }

    // Committed and verified: retire owned backups and holds; problems are warnings.
    let mut warnings = Vec::new();
    for path in &retire {
        if let Err(err) = step("cleanup").and_then(|_| remove_any(path)) {
            warnings.push(format!("could not remove {}: {err}", path.display()));
        }
    }
    for stage in &stages {
        let _ = remove_any(stage);
    }
    let items = plan
        .iter()
        .map(|i| ResolvedItem { from: i.src.clone(), to: i.dest.to_string_lossy().to_string() })
        .collect();
    Ok(TransferResult { items, warnings })
}

pub fn restore_moved_items(items: &[ResolvedItem]) -> Result<(), String> {
    for item in items {
        let original = Path::new(&item.from);
        if original.exists() {
            return Err(format!("undo target already exists: {}", item.from));
        }
        run_cmd_capture("mv", &["-T", "--", &item.to, &item.from])?;
    }
    Ok(())
}

pub fn restore_renamed_items(items: &[ResolvedItem]) -> Result<(), String> {
    let requests = items
        .iter()
        .map(|item| {
            let original_name = Path::new(&item.from)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("bad original name")?
                .to_string();
            Ok((item.to.clone(), original_name))
        })
        .collect::<Result<Vec<_>, String>>()?;
    batch_rename(&requests).map(|_| ())
}

pub fn compress_zip(paths: &[String], dest_dir: &str) -> Result<String, String> {
    if paths.is_empty() {
        return Err("nothing to compress".to_string());
    }
    let stem = Path::new(&paths[0])
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Archive");
    let zip_name = if paths.len() == 1 {
        format!("{stem}.zip")
    } else {
        "Archive.zip".to_string()
    };
    let zip_path = unique_target(dest_dir, &zip_name);
    let zip_path_string = zip_path.to_string_lossy().to_string();

    let mut args = vec!["-r".to_string(), zip_path_string.clone()];
    for path in paths {
        if let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) {
            args.push(name.to_string());
        }
    }

    let output = Command::new("zip")
        .current_dir(dest_dir)
        .args(&args)
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(zip_path_string)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn extract_archive(path: &str, dest_dir: &str) -> Result<(), String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".txz")
        || lower.ends_with(".tar.zst")
        || lower.ends_with(".tzst")
    {
        run_cmd_capture("tar", &["-xf", path, "-C", dest_dir])
    } else if lower.ends_with(".zip") {
        run_cmd_capture("unzip", &["-o", path, "-d", dest_dir])
            .or_else(|_| run_cmd_capture("7z", &["x", "-y", &format!("-o{dest_dir}"), path]))
    } else if is_archive_path(path) {
        run_cmd_capture("7z", &["x", "-y", &format!("-o{dest_dir}"), path]).or_else(|first| {
            run_cmd_capture(
                "unar",
                &["-force-overwrite", "-output-directory", dest_dir, path],
            )
            .map_err(|second| format!("{first}; {second}"))
        })
    } else {
        Err("unsupported archive type".to_string())
    }
}

pub fn empty_trash() -> Result<(), String> {
    run_cmd_capture("gio", &["trash", "--empty"])
}

/// The archive's name without its extension, for naming the folder we extract
/// into. Compound suffixes are handled so "backup.tar.gz" gives "backup", not
/// "backup.tar".
pub fn archive_stem(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let lower = name.to_lowercase();
    for suffix in [
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tar.lzma",
    ] {
        if lower.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_string();
        }
    }
    match name.rfind('.') {
        Some(index) if index > 0 => name[..index].to_string(),
        _ => name,
    }
}

/// Extract into a NEW folder named after the archive, the way Explorer's
/// "Extract All" does. Without this an archive holding loose files scatters
/// them across whatever folder you were looking at, which is impossible to undo
/// by hand. The folder name is uniquified, so extracting twice never overwrites.
pub fn extract_archive_into_new_dir(path: &str, dest_dir: &str) -> Result<String, String> {
    let target = unique_target(dest_dir, &archive_stem(path));
    fs::create_dir_all(&target).map_err(|err| err.to_string())?;
    let target_str = target.to_string_lossy().to_string();
    match extract_archive(path, &target_str) {
        Ok(()) => Ok(target_str),
        Err(err) => {
            // Do not leave an empty folder behind after a failed extraction.
            let _ = fs::remove_dir(&target);
            Err(err)
        }
    }
}

pub fn is_archive_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".txz")
        || lower.ends_with(".tar.zst")
        || lower.ends_with(".tzst")
        || lower.ends_with(".7z")
        || lower.ends_with(".rar")
        || lower.ends_with(".cbr")
        || lower.ends_with(".cbz")
        || lower.ends_with(".gz")
        || lower.ends_with(".bz2")
        || lower.ends_with(".xz")
        || lower.ends_with(".zst")
        || lower.ends_with(".lz")
        || lower.ends_with(".lzma")
        || lower.ends_with(".cab")
        || lower.ends_with(".iso")
        || lower.ends_with(".arj")
        || lower.ends_with(".lzh")
        // Containers that are not "archives" in the everyday sense but which 7z
        // opens perfectly, and which people genuinely want to look inside.
        // .appimage was the one that started this: it is an ELF with a squashfs
        // stapled on, so `file` calls it an executable, but `7z x` extracts it
        // without having to RUN it — which is both safer and works even when the
        // file is not marked executable.
        || lower.ends_with(".appimage")
        || lower.ends_with(".deb")
        || lower.ends_with(".rpm")
        || lower.ends_with(".jar")
        || lower.ends_with(".apk")
        || lower.ends_with(".whl")
        || lower.ends_with(".msi")
        || lower.ends_with(".wim")
        || lower.ends_with(".squashfs")
}

pub fn is_image_path(path: &str) -> bool {
    is_image(Path::new(path))
}

pub fn is_audio_path(path: &str) -> bool {
    has_extension(path, &AUDIO_EXT)
}

pub fn is_video_path(path: &str) -> bool {
    has_extension(path, &VIDEO_EXT)
}

pub fn properties(path: &str) -> Result<FileProperties, String> {
    use std::os::unix::fs::PermissionsExt;

    let md = fs::metadata(path).map_err(|err| err.to_string())?;
    let seconds = |time: std::io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0)
    };
    let path_ref = Path::new(path);
    let items = if md.is_dir() {
        fs::read_dir(path)
            .map(|entries| entries.count() as i64)
            .unwrap_or(-1)
    } else {
        -1
    };
    let kind = if md.is_dir() {
        "Folder".to_string()
    } else {
        type_label(
            path_ref
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path),
        )
    };
    Ok(FileProperties {
        name: path_ref
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string()),
        path: path.to_string(),
        kind,
        size: md.len(),
        is_dir: md.is_dir(),
        modified: seconds(md.modified()),
        created: seconds(md.created()),
        permissions: format!("{:o}", md.permissions().mode() & 0o777),
        items,
    })
}

pub fn entry_summary(entry: &Entry) -> String {
    if entry.is_dir {
        return format!("Folder\n{}", entry.path);
    }
    let kind = kind_label(entry);
    format!(
        "{kind}\n{}\n{}\nModified: {}",
        entry.path,
        fmt_size(entry.size),
        fmt_modified(entry.modified)
    )
}

pub fn kind_label(entry: &Entry) -> String {
    if entry.is_dir {
        "Folder".to_string()
    } else {
        type_label(&entry.name)
    }
}

pub fn fmt_size(mut n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0;
    while n >= 1024 && idx < units.len() - 1 {
        n /= 1024;
        idx += 1;
    }
    format!("{n} {}", units[idx])
}

/// Files modified within this many minutes read as "Just now".
const JUST_NOW_MINUTES: i64 = 2;

fn clock_of(dt: &glib::DateTime) -> String {
    // %l = 12-hour clock, space padded (GLib has no %-I). Trim the pad.
    dt.format("%l:%M %p")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn same_day(a: &glib::DateTime, b: &glib::DateTime) -> bool {
    a.year() == b.year() && a.day_of_year() == b.day_of_year()
}

/// Human-friendly local mtime. Dates use MM/DD/YYYY.
///
/// Formats in-process via glib. This used to shell out to `date(1)` once per
/// entry, which spawned a process for every row in a listing.
pub fn fmt_modified(seconds: i64) -> String {
    fmt_modified_with_now(seconds, glib::DateTime::now_local().ok())
}

/// The formatter with the local "now" supplied, so tests need not depend on
/// the wall clock. `None` means the clock could not be read.
fn fmt_modified_with_now(seconds: i64, now: Option<glib::DateTime>) -> String {
    if seconds <= 0 {
        return "unknown".to_string();
    }
    let Ok(dt) = glib::DateTime::from_unix_local(seconds) else {
        return seconds.to_string();
    };
    let absolute = || {
        dt.format("%m/%d/%Y %l:%M %p")
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|_| seconds.to_string())
    };
    let Some(now) = now else {
        return absolute();
    };

    let diff_minutes = (now.to_unix() - seconds) / 60;
    if diff_minutes < 0 {
        return absolute(); // clock skew / future mtime
    }
    if diff_minutes < JUST_NOW_MINUTES {
        return "Just now".to_string();
    }

    if same_day(&dt, &now) {
        if diff_minutes < 60 {
            return format!("{} min ago", diff_minutes.max(1));
        }
        let hours = diff_minutes / 60;
        if hours < 12 {
            return format!("{hours} hr ago");
        }
        return format!("Today, {}", clock_of(&dt));
    }

    if let Ok(yesterday) = now.add_days(-1) {
        if same_day(&dt, &yesterday) {
            return format!("Yesterday, {}", clock_of(&dt));
        }
    }

    absolute()
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err("invalid name".into());
    }
    Ok(())
}

fn file_contains(path: &Path, query: &str) -> bool {
    use std::io::Read;

    let md = match fs::metadata(path) {
        Ok(md) => md,
        Err(_) => return false,
    };
    if md.len() > 5 * 1024 * 1024 {
        return false;
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut buf = Vec::new();
    if file.take(5 * 1024 * 1024).read_to_end(&mut buf).is_err() {
        return false;
    }
    if buf.contains(&0) {
        return false;
    }
    String::from_utf8_lossy(&buf).to_lowercase().contains(query)
}

fn unique_target(dest_dir: &str, name: &str) -> PathBuf {
    let base = Path::new(dest_dir);
    let first = base.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut n = 2;
    loop {
        let cand = base.join(format!("{} ({}){}", stem, n, ext));
        if !cand.exists() {
            return cand;
        }
        n += 1;
    }
}

fn app_dirs() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        format!("{home}/.local/share/applications"),
        "/usr/local/share/applications".to_string(),
        "/usr/share/applications".to_string(),
        "/var/lib/flatpak/exports/share/applications".to_string(),
        format!("{home}/.local/share/flatpak/exports/share/applications"),
    ]
}

fn whole_disk_of(device: &str) -> String {
    if let Ok(output) = Command::new("lsblk")
        .args(["-no", "PKNAME", device])
        .output()
    {
        let parent = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !parent.is_empty() {
            return format!("/dev/{parent}");
        }
    }
    device.to_string()
}

fn run_cmd_capture(cmd: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("{cmd} failed"))
    } else {
        Err(stderr)
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMG_EXT.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn has_extension(path: &str, extensions: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| extensions.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn icon_name(path: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "aiff"
        | "aif" | "alac" => "audio-x-generic",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "wmv" | "m4v" | "mpg" | "mpeg" | "3gp" | "flv"
        | "ts" => "video-x-generic",
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "svg" | "ico"
        | "avif" | "heic" => "image-x-generic",
        "pdf" => "application-pdf",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz" | "lzma"
        | "cab" => "application-x-archive",
        "txt" | "log" | "md" | "rst" | "nfo" | "text" => "text-x-generic",
        "doc" | "docx" | "odt" | "rtf" => "x-office-document",
        "xls" | "xlsx" | "ods" | "csv" => "x-office-spreadsheet",
        "ppt" | "pptx" | "odp" => "x-office-presentation",
        "html" | "htm" => "text-html",
        "json" => "application-json",
        "sh" | "bash" | "zsh" | "fish" => "text-x-script",
        "py" => "text-x-python",
        "js" | "mjs" | "cjs" | "jsx" | "tsx" => "application-javascript",
        "rs" => "text-rust",
        "appimage" | "bin" | "run" | "exe" | "msi" => "application-x-executable",
        "rpm" => "application-x-rpm",
        "deb" => "application-x-deb",
        _ => "application-octet-stream",
    }
}

fn type_label(name: &str) -> String {
    let ext = PathBuf::from(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_uppercase();
    if ext.is_empty() {
        "File".to_string()
    } else {
        format!("{ext} file")
    }
}

/// The live windows of one process. Each entry owns its state, its widgets and
/// its application hold, and nothing is shared between entries, so closing or
/// driving one window can never touch another (D-19).
pub struct WindowRegistry<S, U, H> {
    entries: Vec<(std::rc::Rc<std::cell::RefCell<S>>, U, H)>,
}

impl<S, U, H> Default for WindowRegistry<S, U, H> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<S, U: Clone, H> WindowRegistry<S, U, H> {
    pub fn register(&mut self, state: &std::rc::Rc<std::cell::RefCell<S>>, ui: U, hold: H) {
        self.entries.push((state.clone(), ui, hold));
    }

    /// Removes only this window. The caller drops the returned hold after it
    /// has released any borrow of the registry.
    pub fn unregister(&mut self, state: &std::rc::Rc<std::cell::RefCell<S>>) -> Option<(U, H)> {
        let index = self
            .entries
            .iter()
            .position(|(entry, _, _)| std::rc::Rc::ptr_eq(entry, state))?;
        let (_, ui, hold) = self.entries.remove(index);
        Some((ui, hold))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn ui_for_state(&self, state: &std::rc::Rc<std::cell::RefCell<S>>) -> Option<U> {
        self.entries
            .iter()
            .find(|(entry, _, _)| std::rc::Rc::ptr_eq(entry, state))
            .map(|(_, ui, _)| ui.clone())
    }

    pub fn state_for_ui(
        &self,
        matches: impl Fn(&U) -> bool,
    ) -> Option<std::rc::Rc<std::cell::RefCell<S>>> {
        self.entries
            .iter()
            .find(|(_, ui, _)| matches(ui))
            .map(|(state, _, _)| state.clone())
    }

    /// Every live window, so a change to shared data can reach all of them.
    pub fn windows(&self) -> Vec<(std::rc::Rc<std::cell::RefCell<S>>, U)> {
        self.entries.iter().map(|(state, ui, _)| (state.clone(), ui.clone())).collect()
    }

    /// The window a desktop request should reuse: the one `is_active` picks,
    /// else the most recently opened.
    pub fn active(
        &self,
        is_active: impl Fn(&U) -> bool,
    ) -> Option<(std::rc::Rc<std::cell::RefCell<S>>, U)> {
        self.entries
            .iter()
            .find(|(_, ui, _)| is_active(ui))
            .or_else(|| self.entries.last())
            .map(|(state, ui, _)| (state.clone(), ui.clone()))
    }
}

/// Minimum gap between two folder-refresh scan starts, and the settle time that
/// lets a writer finish before a scan reads its size.
pub const REFRESH_MIN_GAP_MS: u64 = 500;
pub const REFRESH_SETTLE_MS: u64 = 500;

/// Coalesces directory-monitor events into scans. Pure: the caller passes the
/// clock in milliseconds, so the rules are testable without a main loop.
/// One instance belongs to one monitor generation; a new folder gets a new one.
#[derive(Debug, Default)]
pub struct RefreshCoalescer {
    last_start: Option<u64>,
    timer_pending: bool,
    in_flight: bool,
    dirty: bool,
}

impl RefreshCoalescer {
    fn schedule(&mut self, now: u64) -> u64 {
        self.timer_pending = true;
        let gap = self
            .last_start
            .map_or(0, |start| (start + REFRESH_MIN_GAP_MS).saturating_sub(now));
        gap.max(REFRESH_SETTLE_MS)
    }

    /// A monitor event. `Some(delay)` = arm one timer for that many ms; `None` =
    /// a timer or scan already covers it (an in-flight scan is marked dirty).
    pub fn on_event(&mut self, now: u64) -> Option<u64> {
        if self.in_flight {
            self.dirty = true;
            return None;
        }
        if self.timer_pending {
            return None;
        }
        Some(self.schedule(now))
    }

    /// The armed timer fired: the scan starts now.
    pub fn on_timer(&mut self, now: u64) {
        self.timer_pending = false;
        self.in_flight = true;
        self.dirty = false;
        self.last_start = Some(now);
    }

    /// The scan finished (applied or discarded). `Some(delay)` = one follow-up
    /// scan is owed because events arrived while it ran.
    pub fn on_scan_done(&mut self, now: u64) -> Option<u64> {
        self.in_flight = false;
        if self.dirty {
            self.dirty = false;
            return Some(self.schedule(now));
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshKind {
    /// Same paths and same everything except size/modified: update in place.
    MetadataOnly,
    /// Membership or identity changed: rebuild the rows.
    Structural,
}

/// Compares the current listing with a fresh scan by path identity. Only size
/// and modified time may differ for a metadata-only refresh.
pub fn classify_refresh(current: &[Entry], scanned: &[Entry]) -> RefreshKind {
    if current.len() != scanned.len() {
        return RefreshKind::Structural;
    }
    let by_path: std::collections::HashMap<&str, &Entry> =
        current.iter().map(|entry| (entry.path.as_str(), entry)).collect();
    if by_path.len() != current.len() {
        return RefreshKind::Structural;
    }
    for new in scanned {
        let Some(old) = by_path.get(new.path.as_str()) else {
            return RefreshKind::Structural;
        };
        if old.name != new.name
            || old.is_dir != new.is_dir
            || old.is_image != new.is_image
            || old.icon != new.icon
            || old.metadata_error != new.metadata_error
        {
            return RefreshKind::Structural;
        }
    }
    RefreshKind::MetadataOnly
}

/// Root for throwaway test data: ~/Projects/Scratch/wf-unit-tests, never /tmp.
/// Derived from the build's manifest path, not $HOME, because tests replace HOME.
#[cfg(test)]
pub fn scratch_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let projects = manifest
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "Projects"))
        .expect("the repository lives under ~/Projects");
    let root = projects.join("Scratch").join("wf-unit-tests");
    fs::create_dir_all(&root).expect("create the scratch test root");
    root
}

#[cfg(test)]
mod tests {

    fn proc_state(pid: u32) -> Option<char> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit(')').next()?.trim().chars().next()
    }

    struct KillOnDrop(u32);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = Command::new("kill").arg(self.0.to_string()).status();
        }
    }

    #[test]
    fn spawn_reaped_reaps_short_children_and_returns_promptly_from_long_ones() {
        // A short-lived child must disappear from /proc, not stay defunct,
        // while this process keeps running.
        let mut short = Command::new("sh");
        short.args(["-c", "exit 0"]);
        let short_pid = spawn_reaped(short).expect("short child starts");
        let mut gone = false;
        for _ in 0..100 {
            if proc_state(short_pid).is_none() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(gone, "short child {short_pid} must be fully reaped, state {:?}", proc_state(short_pid));

        // A long-lived child must not be waited for at launch.
        let mut long = Command::new("sleep");
        long.arg("30");
        let started = std::time::Instant::now();
        let long_pid = spawn_reaped(long).expect("long child starts");
        let _cleanup = KillOnDrop(long_pid);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "launch waited {elapsed:?}");
        assert!(
            matches!(proc_state(long_pid), Some(state) if state != 'Z'),
            "long child must still be running, state {:?}",
            proc_state(long_pid)
        );

        // A missing program keeps its launch error.
        let missing = Command::new("wf-no-such-program-d41");
        let err = spawn_reaped(missing).expect_err("missing program fails");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_unix() -> i64 {
        glib::DateTime::now_local().unwrap().to_unix()
    }

    fn refresh_entry(path: &str, size: u64, modified: i64) -> Entry {
        Entry {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            is_dir: false,
            size,
            modified,
            is_image: false,
            icon: "file".into(),
            metadata_error: None,
        }
    }

    /// Fixture: `dest/item` (old) and `src/item` (incoming), as a file or a tree.
    fn replace_fixture(root: &Path, tree: bool) -> (String, String, String) {
        let (src_dir, dest_dir) = (root.join("src"), root.join("dest"));
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        let (src, old) = (src_dir.join("item"), dest_dir.join("item"));
        if tree {
            for (base, tag) in [(&src, "new"), (&old, "old")] {
                fs::create_dir_all(base.join("sub")).unwrap();
                fs::write(base.join("sub/file.txt"), format!("{tag} deep")).unwrap();
                fs::write(base.join(format!("{tag}-only.txt")), tag).unwrap();
            }
        } else {
            fs::write(&src, b"new bytes").unwrap();
            fs::write(&old, b"old bytes").unwrap();
        }
        (
            src.to_string_lossy().to_string(),
            dest_dir.to_string_lossy().to_string(),
            old.to_string_lossy().to_string(),
        )
    }

    fn names_in(dir: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replace_failures_keep_old_destination_and_source_and_leave_no_residue_d23() {
        for tree in [false, true] {
            for is_copy in [true, false] {
                for fail_at in ["prepare", "backup", "place"] {
                    let root = scratch_case("wf-d23-fail");
                    let (src, dest_dir, old) = replace_fixture(&root, tree);
                    let src_before = fingerprint(&src).unwrap();
                    let old_before = fingerprint(&old).unwrap();
                    let step = |name: &str| {
                        if name == fail_at { Err(format!("injected failure at {name}")) } else { Ok(()) }
                    };
                    let result = resolve_paste_with(&[src.clone()], &dest_dir, is_copy, PasteConflict::Replace, &step);
                    // Cross-device MOVE source removal ("remove_source") is not
                    // reachable on one filesystem: untested here, carried to D-24.
                    assert!(result.is_err(), "tree={tree} copy={is_copy} at {fail_at}: {result:?}");
                    assert_eq!(fingerprint(&old).unwrap(), old_before, "old destination intact: tree={tree} copy={is_copy} {fail_at}");
                    assert_eq!(fingerprint(&src).unwrap(), src_before, "source intact: tree={tree} copy={is_copy} {fail_at}");
                    assert_eq!(names_in(&dest_dir), vec!["item".to_string()], "no residue: {fail_at}");
                }
            }
        }
    }

    #[test]
    fn replace_preparation_failure_leaves_old_destination_untouched_d23() {
        for tree in [false, true] {
            let root = scratch_case("wf-d23-prep");
            let (src, dest_dir, old) = replace_fixture(&root, tree);
            let old_before = fingerprint(&old).unwrap();
            let missing = format!("{src}-missing");
            assert!(resolve_paste(&[missing], &dest_dir, true, PasteConflict::Replace).is_err());
            assert_eq!(fingerprint(&old).unwrap(), old_before);
            assert_eq!(names_in(&dest_dir), vec!["item".to_string()]);
        }
    }

    #[test]
    fn replace_success_gives_exact_new_object_and_right_source_presence_d23() {
        for tree in [false, true] {
            for is_copy in [true, false] {
                let root = scratch_case("wf-d23-ok");
                let (src, dest_dir, old) = replace_fixture(&root, tree);
                let snapshot = snapshot_sources(&[src.clone()]).unwrap();
                let items = resolve_paste(&[src.clone()], &dest_dir, is_copy, PasteConflict::Replace).unwrap();
                verify_transfer(&snapshot, &items, is_copy).unwrap();
                assert_eq!(fingerprint(&old).unwrap(), snapshot[0].fp);
                assert_eq!(Path::new(&src).exists(), is_copy);
                if tree {
                    assert!(!Path::new(&old).join("old-only.txt").exists(), "no stale children");
                }
                assert_eq!(names_in(&dest_dir), vec!["item".to_string()], "no stage or backup left");
            }
        }
    }

    /// Fixture for D-24: `src/{a.txt, b/, c.txt}` incoming, `dest/{a.txt, b/}` existing.
    fn batch_fixture(root: &Path) -> (Vec<String>, String) {
        let (src, dest) = (root.join("src"), root.join("dest"));
        for (base, tag) in [(&src, "new"), (&dest, "old")] {
            fs::create_dir_all(base.join("b/sub")).unwrap();
            fs::write(base.join("a.txt"), format!("{tag} a")).unwrap();
            fs::write(base.join("b/sub/file.txt"), format!("{tag} deep")).unwrap();
            fs::write(base.join(format!("b/{tag}-only.txt")), tag).unwrap();
        }
        fs::write(src.join("c.txt"), "new c").unwrap();
        let srcs = ["a.txt", "b", "c.txt"].iter().map(|n| src.join(n).to_string_lossy().to_string()).collect();
        (srcs, dest.to_string_lossy().to_string())
    }

    fn no_owned_names(dir: &Path) -> bool {
        fn walk(dir: &Path) -> bool {
            fs::read_dir(dir).unwrap().all(|e| {
                let e = e.unwrap();
                !e.file_name().to_string_lossy().starts_with(".wf-replace-")
                    && (!e.path().is_dir() || walk(&e.path()))
            })
        }
        walk(dir)
    }

    #[test]
    fn batch_failures_roll_everything_back_exactly_d24() {
        let cases: [(&str, usize); 5] = [("prepare", 2), ("place", 2), ("place", 3), ("verify", 1), ("backup", 2)];
        for conflict in [PasteConflict::KeepBoth, PasteConflict::Replace] {
            for is_copy in [true, false] {
                for force_cross in [false, true] {
                    for (phase, nth) in cases {
                        if phase == "backup" && conflict == PasteConflict::KeepBoth {
                            continue;
                        }
                        let root = scratch_case("wf-d24-fail");
                        let (srcs, dest) = batch_fixture(&root);
                        let before = fingerprint(root.to_str().unwrap()).unwrap();
                        let seen = std::cell::Cell::new(0);
                        let step = |name: &str| {
                            if name == phase {
                                seen.set(seen.get() + 1);
                                if seen.get() == nth {
                                    return Err(format!("injected failure at {phase} #{nth}"));
                                }
                            }
                            Ok(())
                        };
                        let result = transfer_batch(&srcs, &dest, is_copy, conflict, &step, force_cross);
                        let label = format!("{conflict:?} copy={is_copy} cross={force_cross} {phase}#{nth}");
                        assert!(result.is_err(), "{label}: {result:?}");
                        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "exact rollback: {label}");
                        assert!(no_owned_names(&root), "no owned residue: {label}");
                    }
                }
            }
        }
    }

    #[test]
    fn forced_cross_device_move_failures_restore_source_and_old_destination_d24() {
        for phase in ["hold", "place", "verify"] {
            let root = scratch_case("wf-d24-cross");
            let (srcs, dest) = batch_fixture(&root);
            let before = fingerprint(root.to_str().unwrap()).unwrap();
            let step = |name: &str| if name == phase { Err(format!("injected {phase}")) } else { Ok(()) };
            let result = transfer_batch(&srcs, &dest, false, PasteConflict::Replace, &step, true);
            assert!(result.is_err(), "{phase}");
            assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "{phase}");
            assert!(no_owned_names(&root), "{phase}");
        }
    }

    #[test]
    fn cleanup_failure_after_commit_is_a_committed_warning_d24() {
        for is_copy in [true, false] {
            for force_cross in [false, true] {
                let root = scratch_case("wf-d24-warn");
                let (srcs, dest) = batch_fixture(&root);
                let before = snapshot_sources(&srcs).unwrap();
                let step = |name: &str| if name == "cleanup" { Err("injected cleanup".to_string()) } else { Ok(()) };
                let result = transfer_batch(&srcs, &dest, is_copy, PasteConflict::Replace, &step, force_cross)
                    .expect("a checked commit is not an error");
                assert_eq!(result.items.len(), 3);
                verify_transfer(&before, &result.items, is_copy).unwrap();
                assert!(!result.warnings.is_empty(), "cleanup problem is reported as a warning");
                for item in &result.items {
                    assert_eq!(fingerprint(&item.to).unwrap(), before.iter().find(|s| s.path == item.from).unwrap().fp);
                }
            }
        }
    }

    #[test]
    fn successful_batches_verify_and_leave_no_owned_residue_d24() {
        for conflict in [PasteConflict::KeepBoth, PasteConflict::Replace] {
            for is_copy in [true, false] {
                for force_cross in [false, true] {
                    let root = scratch_case("wf-d24-ok");
                    let (srcs, dest) = batch_fixture(&root);
                    let before = snapshot_sources(&srcs).unwrap();
                    let result = transfer_batch(&srcs, &dest, is_copy, conflict, &|_| Ok(()), force_cross).unwrap();
                    assert!(result.warnings.is_empty());
                    assert_eq!(result.items.len(), 3);
                    verify_transfer(&before, &result.items, is_copy).unwrap();
                    assert!(no_owned_names(&root), "{conflict:?} copy={is_copy} cross={force_cross}");
                    if conflict == PasteConflict::KeepBoth {
                        // The old destination stays; incoming items get new names.
                        assert_eq!(fs::read_to_string(Path::new(&dest).join("a.txt")).unwrap(), "old a");
                        assert!(Path::new(&dest).join("a (2).txt").exists());
                    }
                }
            }
        }
    }

    #[test]
    fn broken_symlink_destination_is_an_entry_that_is_never_lost_d24() {
        let setup = |name: &str| {
            let root = scratch_case(name);
            let (src, dest) = (root.join("src"), root.join("dest"));
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(&dest).unwrap();
            fs::write(src.join("link"), "incoming").unwrap();
            std::os::unix::fs::symlink("missing-target", dest.join("link")).unwrap();
            let srcs = vec![src.join("link").to_string_lossy().to_string()];
            (root, srcs, dest.to_string_lossy().to_string())
        };
        // KeepBoth: the broken link stays exact, the incoming file gets a new name.
        for is_copy in [true, false] {
            let (root, srcs, dest) = setup("wf-d24-link-keep");
            let out = transfer_batch(&srcs, &dest, is_copy, PasteConflict::KeepBoth, &|_| Ok(()), false).unwrap();
            let link = Path::new(&dest).join("link");
            assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("missing-target"));
            assert_ne!(out.items[0].to, link.to_string_lossy());
            assert_eq!(fs::read_to_string(&out.items[0].to).unwrap(), "incoming");
            assert!(no_owned_names(&root));
        }
        // Replace, failure after the backup: the exact broken link and source return.
        for is_copy in [true, false] {
            for phase in ["place", "verify"] {
                let (root, srcs, dest) = setup("wf-d24-link-fail");
                let before = fingerprint(root.to_str().unwrap()).unwrap();
                let step = |name: &str| if name == phase { Err("injected".to_string()) } else { Ok(()) };
                assert!(transfer_batch(&srcs, &dest, is_copy, PasteConflict::Replace, &step, false).is_err());
                assert_eq!(fs::read_link(Path::new(&dest).join("link")).unwrap(), PathBuf::from("missing-target"));
                assert_eq!(fs::read_to_string(&srcs[0]).unwrap(), "incoming");
                assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
                assert!(no_owned_names(&root));
            }
        }
        // Replace, success: only the named entry is replaced by the incoming object.
        for is_copy in [true, false] {
            let (root, srcs, dest) = setup("wf-d24-link-ok");
            let out = transfer_batch(&srcs, &dest, is_copy, PasteConflict::Replace, &|_| Ok(()), false).unwrap();
            let link = Path::new(&dest).join("link");
            assert!(!fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert_eq!(fs::read_to_string(&link).unwrap(), "incoming");
            assert_eq!(out.items.len(), 1);
            assert_eq!(names_in(&dest), vec!["link".to_string()]);
            assert!(no_owned_names(&root));
        }
    }

    #[test]
    fn scratch_case_disappears_when_a_test_panics() {
        let path = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let seen = path.clone();
        let result = std::panic::catch_unwind(move || {
            let dir = scratch_case("wf-unwind");
            fs::write(dir.join("marker"), b"x").unwrap();
            *seen.lock().unwrap() = Some(dir.to_path_buf());
            panic!("deliberate assertion failure");
        });
        assert!(result.is_err());
        let dir = path.lock().unwrap().clone().expect("fixture was created");
        assert!(!dir.exists(), "the fixture is removed while unwinding");
    }

    #[test]
    fn refresh_coalescer_limits_scans_and_never_overlaps_d22() {
        let mut c = RefreshCoalescer::default();
        // A burst of events yields one timer, exactly 500 ms after the first.
        assert_eq!(c.on_event(1000), Some(500));
        assert_eq!(c.on_event(1010), None);
        assert_eq!(c.on_event(1200), None);
        c.on_timer(1500);
        // Events during the scan mark it dirty and start nothing.
        assert_eq!(c.on_event(1510), None);
        assert_eq!(c.on_event(1520), None);
        // The follow-up is one scan, never sooner than 500 ms after the last start.
        let owed = c.on_scan_done(1550).expect("one follow-up is owed");
        assert!(1550 + owed >= 1500 + REFRESH_MIN_GAP_MS);
        assert_eq!(c.on_event(1600), None);
        c.on_timer(1550 + owed);
        // No events while it ran: nothing owed.
        assert_eq!(c.on_scan_done(2100), None);
        // A later event still waits at least 500 ms from the last start (2050).
        let later = c.on_event(2150).expect("a new timer");
        assert!(2150 + later >= 2050 + REFRESH_MIN_GAP_MS && later >= 500);
        c.on_timer(2150 + later);
        assert_eq!(c.on_scan_done(2700), None);
        // Far in the future: the 500 ms first-event delay only.
        assert_eq!(c.on_event(9000), Some(500));
    }

    #[test]
    fn refresh_scans_start_at_least_500ms_apart_under_a_write_storm_d22() {
        let mut c = RefreshCoalescer::default();
        let mut starts: Vec<u64> = Vec::new();
        let mut timer: Option<u64> = None;
        let mut scan_end: Option<u64> = None;
        for now in 0..6000u64 {
            if timer == Some(now) {
                timer = None;
                c.on_timer(now);
                starts.push(now);
                scan_end = Some(now + 20);
            }
            if scan_end == Some(now) {
                scan_end = None;
                if let Some(delay) = c.on_scan_done(now) {
                    timer = Some(now + delay);
                }
            }
            if now % 7 == 0 {
                if let Some(delay) = c.on_event(now) {
                    timer = Some(now + delay);
                }
            }
        }
        assert!(starts.len() >= 5, "scans keep happening: {starts:?}");
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] >= REFRESH_MIN_GAP_MS, "{starts:?}");
        }
    }

    #[test]
    fn refresh_classification_keeps_identity_and_flags_structure_d22() {
        let current = vec![refresh_entry("/d/a", 1, 10), refresh_entry("/d/b", 2, 20)];
        let grown = vec![refresh_entry("/d/b", 999, 99), refresh_entry("/d/a", 5, 50)];
        assert_eq!(classify_refresh(&current, &grown), RefreshKind::MetadataOnly);
        assert_eq!(classify_refresh(&current, &current), RefreshKind::MetadataOnly);
        let created = vec![current[0].clone(), current[1].clone(), refresh_entry("/d/c", 1, 1)];
        assert_eq!(classify_refresh(&current, &created), RefreshKind::Structural);
        assert_eq!(classify_refresh(&created, &current), RefreshKind::Structural);
        let renamed = vec![refresh_entry("/d/a", 1, 10), refresh_entry("/d/z", 2, 20)];
        assert_eq!(classify_refresh(&current, &renamed), RefreshKind::Structural);
        let mut retyped = current.clone();
        retyped[0].is_dir = true;
        assert_eq!(classify_refresh(&current, &retyped), RefreshKind::Structural);
        let mut broken = current.clone();
        broken[1].metadata_error = Some("gone".into());
        assert_eq!(classify_refresh(&current, &broken), RefreshKind::Structural);
    }

    #[test]
    fn numbered_folder_is_explorer_numbered_and_never_overwrites_d21() {
        let dir = test_dir("numbered-folder");
        let parent = dir.to_str().unwrap();
        let first = make_numbered_dir(parent, "New Folder").unwrap();
        assert_eq!(first, dir.join("New Folder").to_string_lossy());
        // A file and a folder already own the next names; a marker proves no overwrite.
        fs::write(dir.join("New Folder (2)"), "keep").unwrap();
        fs::create_dir(dir.join("New Folder (3)")).unwrap();
        fs::write(dir.join("New Folder (3)").join("marker"), "keep").unwrap();
        let next = make_numbered_dir(parent, "New Folder").unwrap();
        assert_eq!(next, dir.join("New Folder (4)").to_string_lossy());
        assert!(dir.join("New Folder (4)").is_dir());
        assert_eq!(fs::read_to_string(dir.join("New Folder (2)")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(dir.join("New Folder (3)").join("marker")).unwrap(), "keep");
        let fifth = make_numbered_dir(parent, "New Folder").unwrap();
        assert_eq!(fifth, dir.join("New Folder (5)").to_string_lossy());
        assert!(make_numbered_dir(parent, "a/b").is_err());
        assert!(make_numbered_dir(dir.join("missing").to_str().unwrap(), "New Folder").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_path_refuses_illegal_names() {
        let dir = test_dir("rename-illegal");
        let file = dir.join("a.txt");
        fs::write(&file, "a").unwrap();
        for bad in ["", "   ", ".", "..", "x/y"] {
            assert!(rename_path(file.to_str().unwrap(), bad).is_err(), "{bad:?} must be refused");
        }
        assert!(file.exists(), "the source is untouched");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_path_never_overwrites_an_existing_sibling() {
        let dir = test_dir("rename-collision");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "AAA").unwrap();
        fs::write(&b, "BBB").unwrap();
        let err = rename_path(a.to_str().unwrap(), "b.txt").unwrap_err();
        assert!(err.contains("already exists"), "reason names the collision: {err}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "AAA");
        assert_eq!(fs::read_to_string(&b).unwrap(), "BBB");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_path_same_name_is_a_no_op_and_real_rename_moves_the_file() {
        let dir = test_dir("rename-real");
        let a = dir.join("a.txt");
        fs::write(&a, "AAA").unwrap();
        assert_eq!(rename_path(a.to_str().unwrap(), "a.txt").unwrap(), a.to_string_lossy());
        assert!(a.exists());
        let to = rename_path(a.to_str().unwrap(), "café.txt").unwrap();
        assert!(!a.exists(), "old path gone");
        assert_eq!(fs::read_to_string(&to).unwrap(), "AAA");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_dir_keeps_broken_links_visible_with_an_honest_metadata_error() {
        let dir = test_dir("broken-link");
        let link = dir.join("missing-video.mp4");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("does-not-exist"), &link).unwrap();

        let cancelled = AtomicBool::new(false);
        let listing = list_dir_cancellable(dir.to_str().unwrap(), false, &cancelled).unwrap();
        let entry = listing
            .entries
            .iter()
            .find(|entry| entry.name == "missing-video.mp4")
            .expect("a metadata failure must not hide the filename");
        assert!(entry.metadata_error.is_some());
        assert_eq!(listing.metadata_unavailable, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelled_directory_load_stops_before_opening_the_folder() {
        let dir = test_dir("cancelled-listing");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            list_dir_cancellable(dir.to_str().unwrap(), false, &cancelled),
            Err(ListDirError::Cancelled)
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uri_list_is_crlf_separated_percent_encoded_file_uris() {
        // Browsers require file:// URIs, CRLF separators, and percent-encoded
        // spaces — a bare path or LF separator makes the drop silently fail.
        let paths = vec![
            "/home/zito/Downloads/a b.png".to_string(),
            "/home/zito/x.jpg".to_string(),
        ];
        let out = uri_list_for_paths(&paths);
        let lines: Vec<&str> = out.split("\r\n").collect();
        assert_eq!(lines.len(), 2, "CRLF-separated, got {out:?}");
        assert!(
            lines[0].starts_with("file:///"),
            "need file:// scheme: {out:?}"
        );
        assert!(
            lines[0].contains("a%20b.png"),
            "space must be percent-encoded: {out:?}"
        );
        assert!(lines[1].ends_with("x.jpg"));
        assert!(!out.contains('\n') || out.contains("\r\n"), "no bare LF");
    }

    #[test]
    fn trash_paths_batch_deletes_without_per_file_subprocess() {
        // Reproduces the batch-delete freeze: this used to spawn one `gio trash`
        // process per file and block the UI thread on each.
        //
        // Must live on the real $HOME filesystem: GIO refuses to trash from tmpfs
        // ("system internal mounts"), so /tmp cannot exercise this path.
        let home = std::env::var("HOME").expect("HOME");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let stem = format!("wf-trashtest-{}-{nanos}", std::process::id());
        let dir = PathBuf::from(&home).join(".cache").join(&stem);
        fs::create_dir_all(&dir).unwrap();

        let mut paths = Vec::new();
        let mut names = Vec::new();
        for i in 0..25 {
            let name = format!("{stem}-{i}.png");
            let file = dir.join(&name);
            fs::write(&file, b"x").unwrap();
            paths.push(file.to_string_lossy().to_string());
            names.push(name);
        }

        let started = std::time::Instant::now();
        let result = trash_paths(&paths);
        let elapsed = started.elapsed();

        // Clean our files back out of the real Trash regardless of outcome.
        let trash_files = PathBuf::from(&home).join(".local/share/Trash/files");
        let trash_info = PathBuf::from(&home).join(".local/share/Trash/info");
        for name in &names {
            fs::remove_file(trash_files.join(name)).ok();
            fs::remove_file(trash_info.join(format!("{name}.trashinfo"))).ok();
        }
        fs::remove_dir_all(&dir).ok();

        result.expect("batch trash should succeed");
        for path in &paths {
            assert!(!Path::new(path).exists(), "{path} should be trashed");
        }
        // 25 sequential `gio trash` spawns cost seconds. The library call is ~1ms
        // each; anything near the old cost means the subprocess crept back in.
        assert!(
            elapsed.as_millis() < 400,
            "batch trash of 25 files took {elapsed:?} — too slow, is it spawning processes again?"
        );
        eprintln!("trashed 25 files in {elapsed:?}");
    }

    #[test]
    fn fmt_modified_is_cheap_enough_for_large_listings() {
        // Every row in a listing calls this, and listing happens on the UI thread
        // (navigate_to -> list_dir -> build rows), including the refresh after a
        // batch delete. The old `date(1)` spawn cost ~5ms/file here, so a folder of
        // 500 images froze the window for seconds.
        let now = now_unix();
        let started = std::time::Instant::now();
        for i in 0..500i64 {
            let _ = fmt_modified(now - 3600 - (i % 40) * 86_400);
        }
        let elapsed = started.elapsed();
        eprintln!("500 fmt_modified calls in {elapsed:?}");
        assert!(
            elapsed.as_millis() < 150,
            "500 fmt_modified calls took {elapsed:?} — did the subprocess creep back in?"
        );
    }

    #[test]
    fn fmt_modified_says_just_now_for_fresh_files() {
        assert_eq!(fmt_modified(now_unix() - 30), "Just now");
    }

    #[test]
    fn fmt_modified_counts_minutes_then_hours() {
        // A fixed mid-afternoon "now", so no interval crosses local midnight.
        let now = glib::DateTime::new(&glib::TimeZone::local(), 2026, 6, 15, 14, 0, 0.0).unwrap();
        let at = |back: i64| fmt_modified_with_now(now.to_unix() - back, Some(now.clone()));
        assert_eq!(at(10 * 60), "10 min ago");
        assert_eq!(at(3 * 3600), "3 hr ago");
    }

    #[test]
    fn fmt_modified_across_midnight_says_yesterday() {
        // Ten minutes old but on the prior local day: the existing policy.
        let now = glib::DateTime::new(&glib::TimeZone::local(), 2026, 6, 15, 0, 5, 0.0).unwrap();
        assert_eq!(fmt_modified_with_now(now.to_unix() - 10 * 60, Some(now.clone())), "Yesterday, 11:55 PM");
    }

    #[test]
    fn fmt_modified_uses_slashes_and_never_dashes_for_old_dates() {
        // 2021-03-05T12:00:00Z — comfortably older than "yesterday" in any tz.
        let out = fmt_modified(1614945600);
        assert!(
            out.contains('/'),
            "expected MM/DD/YYYY slashes, got {out:?}"
        );
        assert!(
            !out.contains('-'),
            "date must not contain dashes, got {out:?}"
        );
        assert!(out.contains("2021"), "expected the year, got {out:?}");
    }

    #[test]
    fn fmt_modified_absolute_form_is_not_double_spaced() {
        // %l space-pads single-digit hours; that padding must be collapsed so the
        // label does not render with a gap and get needlessly ellipsized.
        let out = fmt_modified(1614945600);
        assert!(
            !out.contains("  "),
            "collapsed spacing expected, got {out:?}"
        );
        assert_eq!(out.trim(), out, "no leading/trailing space, got {out:?}");
    }

    #[test]
    fn fmt_modified_handles_unknown() {
        assert_eq!(fmt_modified(0), "unknown");
        assert_eq!(fmt_modified(-5), "unknown");
    }

    fn test_dir(name: &str) -> ScratchCase {
        scratch_case(&format!("wayfinder-native-{name}"))
    }

    /// Builds a real on-disk trash entry (files/ + info/) and returns its trash dir.
    fn trash_entry(root: &Path, stem: &str, original: &Path, date: &str, body: &[u8]) {
        let trash = root.join("Trash");
        fs::create_dir_all(trash.join("files")).unwrap();
        fs::create_dir_all(trash.join("info")).unwrap();
        fs::write(trash.join("files").join(stem), body).unwrap();
        let encoded: String = original
            .to_string_lossy()
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        fs::write(
            trash.join("info").join(format!("{stem}.trashinfo")),
            format!("[Trash Info]\nPath={encoded}\nDeletionDate={date}\n"),
        )
        .unwrap();
    }

    fn trash_left(root: &Path) -> usize {
        let count = |sub: &str| fs::read_dir(root.join("Trash").join(sub)).unwrap().count();
        count("files") + count("info")
    }

    #[test]
    fn restore_from_trash_repeats_for_spaced_percent_name_without_daemon() {
        let root = test_dir("restore-repeat");
        let dest = root.join("F");
        fs::create_dir_all(&dest).unwrap();
        let original = dest.join("wf 50% fixture.txt");
        for round in 0..3 {
            let body = format!("body {round}");
            trash_entry(&root, "wf 50% fixture.txt", &original, "2026-01-01T00:00:00", body.as_bytes());
            restore_from_trash_in(&root.join("Trash"), original.to_str().unwrap()).unwrap();
            assert_eq!(fs::read_to_string(&original).unwrap(), body);
            assert_eq!(trash_left(&root), 0);
            fs::remove_file(&original).unwrap();
        }
        // newest exact match is chosen; the older entry stays untouched
        trash_entry(&root, "a", &original, "2026-01-01T00:00:00", b"old");
        trash_entry(&root, "a_2", &original, "2026-02-01T00:00:00", b"new");
        restore_from_trash_in(&root.join("Trash"), original.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert!(root.join("Trash/files/a").exists() && root.join("Trash/info/a.trashinfo").exists());
        assert_eq!(trash_left(&root), 2);
        // explicit Trash/files/<name> form uses the same resolver
        fs::remove_file(&original).unwrap();
        let explicit = format!("{}/Trash/files/a", root.display());
        restore_from_trash_in(&root.join("Trash"), &explicit).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"old");
        assert_eq!(trash_left(&root), 0);
    }

    #[test]
    fn restore_from_trash_restores_directory_with_nested_file() {
        let root = test_dir("restore-dir");
        let dest = root.join("F");
        fs::create_dir_all(&dest).unwrap();
        let original = dest.join("folder");
        trash_entry(&root, "folder", &original, "2026-01-01T00:00:00", b"");
        let payload = root.join("Trash/files/folder");
        fs::remove_file(&payload).unwrap();
        fs::create_dir_all(payload.join("sub")).unwrap();
        fs::write(payload.join("sub/n.txt"), b"nested").unwrap();
        restore_from_trash_in(&root.join("Trash"), original.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(original.join("sub/n.txt")).unwrap(), b"nested");
        assert_eq!(trash_left(&root), 0);
    }

    #[test]
    fn restore_from_trash_never_overwrites_and_reports_bad_metadata() {
        let root = test_dir("restore-refuse");
        let dest = root.join("F");
        fs::create_dir_all(&dest).unwrap();
        let trash = root.join("Trash");
        let original = dest.join("x.txt");

        trash_entry(&root, "x.txt", &original, "2026-01-01T00:00:00", b"trashed");
        fs::write(&original, b"existing").unwrap();
        let err = restore_from_trash_in(&trash, original.to_str().unwrap()).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(fs::read(&original).unwrap(), b"existing");
        assert_eq!(trash_left(&root), 2);
        fs::remove_file(&original).unwrap();

        std::os::unix::fs::symlink(dest.join("nowhere"), &original).unwrap();
        let err = restore_from_trash_in(&trash, original.to_str().unwrap()).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert!(fs::symlink_metadata(&original).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(trash.join("files/x.txt")).unwrap(), b"trashed");
        fs::remove_file(&original).unwrap();

        let err = restore_from_trash_in(&trash, dest.join("absent").to_str().unwrap()).unwrap_err();
        assert_eq!(err, format!("not found in trash: {}/absent", dest.display()));

        fs::write(trash.join("info/bad.trashinfo"), "[Trash Info]\nDeletionDate=x\n").unwrap();
        fs::write(trash.join("files/bad"), b"b").unwrap();
        let sel = format!("{}/files/bad", trash.display());
        let err = restore_from_trash_in(&trash, &sel).unwrap_err();
        assert!(err.starts_with("malformed trash metadata"), "{err}");

        fs::remove_file(trash.join("files/x.txt")).unwrap();
        let err = restore_from_trash_in(&trash, original.to_str().unwrap()).unwrap_err();
        assert!(err.starts_with("trash payload missing"), "{err}");

        let gone = dest.join("gone-dir/y.txt");
        trash_entry(&root, "y.txt", &gone, "2026-01-01T00:00:00", b"y");
        let err = restore_from_trash_in(&trash, gone.to_str().unwrap()).unwrap_err();
        assert!(err.starts_with("destination folder is missing"), "{err}");
        assert!(trash.join("files/y.txt").exists() && trash.join("info/y.txt.trashinfo").exists());
        assert!(trash.join("files/bad").exists());
    }

    #[test]
    fn restore_from_trash_selector_is_confined_to_this_trash() {
        let root = test_dir("restore-confine");
        let dest = root.join("F");
        fs::create_dir_all(&dest).unwrap();
        let trash = root.join("Trash");
        let original = dest.join("c.txt");
        trash_entry(&root, "c.txt", &original, "2026-01-01T00:00:00", b"c");
        // a lookalike Trash elsewhere, a nested path and dot names are not selectors
        let lookalike = root.join("Other/Trash/files/c.txt");
        for sel in [
            lookalike.to_str().unwrap().to_string(),
            format!("{}/files/sub/c.txt", trash.display()),
            format!("{}/files/..", trash.display()),
            format!("{}/files/.", trash.display()),
            format!("{}/files/../info/c.txt", trash.display()),
        ] {
            let err = restore_from_trash_in(&trash, &sel).unwrap_err();
            assert!(err.starts_with("not found in trash"), "{sel}: {err}");
        }
        assert_eq!(trash_left(&root), 2);
        // a symlinked .trashinfo is refused
        fs::rename(trash.join("info/c.txt.trashinfo"), root.join("real.info")).unwrap();
        std::os::unix::fs::symlink(root.join("real.info"), trash.join("info/c.txt.trashinfo")).unwrap();
        let sel = format!("{}/files/c.txt", trash.display());
        let err = restore_from_trash_in(&trash, &sel).unwrap_err();
        assert!(err.starts_with("trash metadata is not a regular file"), "{err}");
        assert!(!original.exists() && trash.join("files/c.txt").exists());
    }

    #[test]
    fn restore_from_trash_rolls_back_when_metadata_removal_fails() {
        let root = test_dir("restore-rollback");
        let dest = root.join("F");
        fs::create_dir_all(&dest).unwrap();
        let trash = root.join("Trash");
        let original = dest.join("r.txt");
        trash_entry(&root, "r.txt", &original, "2026-01-01T00:00:00", b"payload");
        let info = trash.join("info/r.txt.trashinfo");
        let payload = trash.join("files/r.txt");

        let fail = |_: &Path| -> std::io::Result<()> {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "injected"))
        };
        let err = restore_with(&trash, original.to_str().unwrap(), &fail).unwrap_err();
        assert!(err.contains("injected") && err.contains("item left in trash at"), "{err}");
        assert!(err.contains(payload.to_str().unwrap()), "{err}");
        assert!(!original.exists());
        assert_eq!(fs::read(&payload).unwrap(), b"payload");
        assert!(info.exists());

        // rollback itself collides: the payload path is re-occupied during removal
        let occupied = payload.clone();
        let fail_and_occupy = move |_: &Path| -> std::io::Result<()> {
            fs::write(&occupied, b"intruder")?;
            Err(std::io::Error::new(std::io::ErrorKind::Other, "injected"))
        };
        let err = restore_with(&trash, original.to_str().unwrap(), &fail_and_occupy).unwrap_err();
        assert!(err.contains("could not move back"), "{err}");
        assert!(err.contains(original.to_str().unwrap()), "{err}");
        assert!(err.contains(payload.to_str().unwrap()), "{err}");
        assert_eq!(fs::read(&original).unwrap(), b"payload");
        assert_eq!(fs::read(&payload).unwrap(), b"intruder");
        assert!(info.exists());
    }

    #[test]
    fn batch_rename_swaps_names_and_undoes() {
        let dir = test_dir("swap");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "alpha").unwrap();
        fs::write(&b, "bravo").unwrap();

        let done = batch_rename(&[
            (a.to_string_lossy().to_string(), "b.txt".to_string()),
            (b.to_string_lossy().to_string(), "a.txt".to_string()),
        ])
        .unwrap();

        assert_eq!(fs::read_to_string(&a).unwrap(), "bravo");
        assert_eq!(fs::read_to_string(&b).unwrap(), "alpha");

        restore_renamed_items(&done).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(&b).unwrap(), "bravo");

        fs::remove_dir_all(dir).unwrap();
    }

    fn no_rename_residue(dir: &Path) -> bool {
        fn walk(dir: &Path) -> bool {
            fs::read_dir(dir).unwrap().all(|e| {
                let e = e.unwrap();
                !e.file_name().to_string_lossy().starts_with(".wayfinder-rename-")
                    && (!e.file_type().unwrap().is_dir() || walk(&e.path()))
            })
        }
        walk(dir)
    }

    fn rename_fixture(root: &Path) -> Vec<(String, String)> {
        fs::create_dir_all(root.join("tree/inner")).unwrap();
        fs::write(root.join("tree/inner/deep.txt"), "deep").unwrap();
        fs::write(root.join("a.txt"), "alpha").unwrap();
        std::os::unix::fs::symlink("missing-target", root.join("link")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other/o.txt"), "oo").unwrap();
        let p = |n: &str| root.join(n).to_string_lossy().to_string();
        vec![
            (p("a.txt"), "a2.txt".into()),
            (p("tree"), "tree2".into()),
            (p("link"), "link2".into()),
            (p("other/o.txt"), "o2.txt".into()),
        ]
    }

    #[test]
    fn batch_rename_failures_restore_everything_exactly_d25() {
        for (phase, nth) in [("stage", 2), ("place", 2), ("place", 3), ("stage", 4), ("place", 4)] {
            let root = scratch_case("wf-d25-fail");
            let items = rename_fixture(&root);
            let before = fingerprint(root.to_str().unwrap()).unwrap();
            let seen = std::cell::Cell::new(0);
            let step = |name: &str| {
                if name == phase {
                    seen.set(seen.get() + 1);
                    if seen.get() == nth {
                        return Err(format!("injected failure at {phase} #{nth}"));
                    }
                }
                Ok(())
            };
            let result = batch_rename_with(&items, &step);
            assert!(result.is_err(), "{phase}#{nth}: {result:?}");
            assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "exact: {phase}#{nth}");
            assert!(no_rename_residue(&root), "residue: {phase}#{nth}");
            assert!(fs::symlink_metadata(root.join("link")).is_ok());
        }
    }

    #[test]
    fn batch_rename_swap_and_cycle_place_failures_roll_back_d25() {
        for cycle in [false, true] {
            for nth in [2, 3] {
                if !cycle && nth == 3 {
                    continue;
                }
                let root = scratch_case("wf-d25-cyc-fail");
                fs::write(root.join("a"), "A file").unwrap();
                fs::create_dir(root.join("b")).unwrap();
                fs::write(root.join("b/x"), "B tree").unwrap();
                std::os::unix::fs::symlink("c-target", root.join("c")).unwrap();
                let before = fingerprint(root.to_str().unwrap()).unwrap();
                let p = |n: &str| root.join(n).to_string_lossy().to_string();
                let items = if cycle {
                    vec![(p("a"), "b".into()), (p("b"), "c".into()), (p("c"), "a".into())]
                } else {
                    vec![(p("a"), "b".into()), (p("b"), "a".into())]
                };
                let seen = std::cell::Cell::new(0);
                let step = |name: &str| {
                    if name == "place" {
                        seen.set(seen.get() + 1);
                        if seen.get() == nth {
                            return Err("injected".to_string());
                        }
                    }
                    Ok(())
                };
                assert!(batch_rename_with(&items, &step).is_err());
                assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "cycle={cycle} place#{nth}");
                assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "A file");
                assert_eq!(fs::read_to_string(root.join("b/x")).unwrap(), "B tree");
                assert_eq!(fs::read_link(root.join("c")).unwrap().to_string_lossy(), "c-target");
                assert!(no_rename_residue(&root));
            }
        }
    }

    #[test]
    fn batch_rename_cycle_succeeds_and_undoes_d25() {
        let root = scratch_case("wf-d25-cycle");
        fs::write(root.join("a"), "A").unwrap();
        fs::write(root.join("b"), "B").unwrap();
        fs::create_dir(root.join("c")).unwrap();
        fs::write(root.join("c/x"), "X").unwrap();
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        let p = |n: &str| root.join(n).to_string_lossy().to_string();
        let done = batch_rename(&[(p("a"), "b".into()), (p("b"), "c".into()), (p("c"), "a".into())]).unwrap();
        assert_eq!(fs::read_to_string(root.join("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(root.join("c")).unwrap(), "B");
        assert_eq!(fs::read_to_string(root.join("a/x")).unwrap(), "X");
        assert!(no_rename_residue(&root));
        restore_renamed_items(&done).unwrap();
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
        assert!(no_rename_residue(&root));
    }

    #[test]
    fn batch_rename_broken_links_d25() {
        let root = scratch_case("wf-d25-link");
        let p = |n: &str| root.join(n).to_string_lossy().to_string();
        std::os::unix::fs::symlink("gone", root.join("l")).unwrap();
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        let done = batch_rename(&[(p("l"), "l2".into())]).unwrap();
        assert_eq!(fs::read_link(root.join("l2")).unwrap().to_string_lossy(), "gone");
        assert!(fs::symlink_metadata(root.join("l")).is_err());
        restore_renamed_items(&done).unwrap();
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);

        // unrelated broken-link destination is occupied
        fs::write(root.join("f"), "F").unwrap();
        std::os::unix::fs::symlink("gone2", root.join("busy")).unwrap();
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        assert!(batch_rename(&[(p("f"), "busy".into())]).is_err());
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
        assert!(no_rename_residue(&root));

        // cycle containing a broken link
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        let done = batch_rename(&[(p("f"), "busy".into()), (p("busy"), "f".into())]).unwrap();
        assert_eq!(fs::read_to_string(root.join("busy")).unwrap(), "F");
        assert_eq!(fs::read_link(root.join("f")).unwrap().to_string_lossy(), "gone2");
        restore_renamed_items(&done).unwrap();
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
        assert!(no_rename_residue(&root));
    }

    #[test]
    fn batch_rename_rejects_duplicates_and_noops_d25() {
        let root = scratch_case("wf-d25-dup");
        let p = |n: &str| root.join(n).to_string_lossy().to_string();
        fs::write(root.join("a"), "A").unwrap();
        fs::write(root.join("b"), "B").unwrap();
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        assert!(batch_rename(&[(p("a"), "z".into()), (p("b"), "z".into())]).is_err());
        assert!(batch_rename(&[(p("a"), "y".into()), (p("a"), "z".into())]).is_err());
        assert!(batch_rename(&[(p("missing"), "q".into())]).is_err());
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
        assert!(batch_rename(&[(p("a"), "a".into())]).unwrap().is_empty());
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
    }

    fn lsblk_json(entries: &[(&str, serde_json::Value)]) -> String {
        let nodes: Vec<_> = entries
            .iter()
            .map(|(path, mounts)| serde_json::json!({"path": path, "mountpoints": mounts}))
            .collect();
        serde_json::json!({ "blockdevices": nodes }).to_string()
    }

    #[test]
    fn mount_result_needs_a_verified_mountpoint_d26() {
        let root = scratch_case("wf-d26 mount");
        let spaced = root.join("USB DRIVE");
        fs::create_dir(&spaced).unwrap();
        let file = root.join("plain.txt");
        fs::write(&file, "x").unwrap();
        let s = |p: &Path| p.to_string_lossy().to_string();
        let ok: &dyn Fn(&str) -> MountRun = &|_| Ok(Ok(()));
        let dev = "/dev/sdx1";
        let with = |json: String| match mount_drive_with(dev, ok, &move |_| Ok(json.clone())) {
            Ok(Mounted::New(path)) => Ok(path),
            Ok(Mounted::Already(path)) => panic!("a zero exit is a new mount: {path}"),
            Err(err) => Err(err),
        };

        // nonzero exit with nothing mounted keeps its stderr
        let failed = mount_drive_with(
            dev,
            &|_| Ok(Err("Error mounting: busy".into())),
            &|_| Ok(lsblk_json(&[(dev, serde_json::json!([null]))])),
        );
        assert_eq!(failed.unwrap_err(), "Error mounting: busy");
        assert!(mount_drive_with(dev, &|_| Err("no udisksctl".into()), &|_| panic!()).is_err());

        // zero exit, no mountpoint in system state: rejected
        let err = with(lsblk_json(&[(dev, serde_json::json!([null]))])).unwrap_err();
        assert!(err.contains("no usable mountpoint"), "{err}");
        assert!(with("not json".into()).is_err());
        assert!(mount_drive_with(dev, ok, &|_| Err("lsblk gone".into())).is_err());

        // stdout wording is never consulted: valid device mountpoint accepted
        assert_eq!(with(lsblk_json(&[(dev, serde_json::json!([s(&spaced)]))])).unwrap(), s(&spaced));

        // bad candidates
        for bad in ["", "relative/dir", &s(&root.join("missing")), &s(&file)] {
            assert!(with(lsblk_json(&[(dev, serde_json::json!([bad]))])).is_err(), "{bad}");
        }

        // a mountpoint belonging to another device only
        let other = lsblk_json(&[("/dev/sdy1", serde_json::json!([s(&spaced)])), (dev, serde_json::json!([null]))]);
        assert!(with(other).is_err());
        assert!(with(lsblk_json(&[("/dev/sdy1", serde_json::json!([s(&spaced)]))])).is_err());

        // ambiguous: several distinct mountpoints
        let two = lsblk_json(&[(dev, serde_json::json!([s(&spaced), s(&root)]))]);
        assert!(with(two).unwrap_err().contains("several"));
        // same path repeated is one candidate
        assert!(with(lsblk_json(&[(dev, serde_json::json!([s(&spaced), s(&spaced)]))])).is_ok());

        // nested child device
        let nested = serde_json::json!({"blockdevices": [{"path": "/dev/sdx", "mountpoints": [null],
            "children": [{"path": dev, "mountpoints": [s(&spaced)]}]}]})
        .to_string();
        assert_eq!(with(nested).unwrap(), s(&spaced));
    }

    #[test]
    fn already_mounted_is_recovered_from_verified_state_only_d26() {
        let root = scratch_case("wf-d26 already");
        let spaced = root.join("USB DRIVE");
        fs::create_dir(&spaced).unwrap();
        let file = root.join("plain.txt");
        fs::write(&file, "x").unwrap();
        let s = |p: &Path| p.to_string_lossy().to_string();
        let dev = "/dev/sdx1";
        let refused: &dyn Fn(&str) -> MountRun =
            &|_| Ok(Err("Error mounting: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted".into()));
        let after = |json: String| mount_drive_with(dev, refused, &move |_| Ok(json.clone()));

        // zero exit plus one verified mountpoint is a new mount
        let new = mount_drive_with(dev, &|_| Ok(Ok(())), &|_| Ok(lsblk_json(&[(dev, serde_json::json!([s(&spaced)]))])));
        assert_eq!(new, Ok(Mounted::New(s(&spaced))));

        // nonzero exit plus one verified mountpoint is already mounted
        assert_eq!(
            after(lsblk_json(&[(dev, serde_json::json!([s(&spaced)]))])),
            Ok(Mounted::Already(s(&spaced)))
        );

        // the command's wording is irrelevant: any nonzero exit with the same state recovers
        let other = mount_drive_with(
            dev,
            &|_| Ok(Err("something else entirely".into())),
            &|_| Ok(lsblk_json(&[(dev, serde_json::json!([s(&spaced)]))])),
        );
        assert_eq!(other, Ok(Mounted::Already(s(&spaced))));

        // no / invalid / multiple mountpoints keep the original error
        let original = "Error mounting: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted";
        assert_eq!(after(lsblk_json(&[(dev, serde_json::json!([null]))])).unwrap_err(), original);
        for bad in ["", "relative/dir", &s(&root.join("missing")), &s(&file)] {
            assert_eq!(after(lsblk_json(&[(dev, serde_json::json!([bad]))])).unwrap_err(), original, "{bad}");
        }
        let two = lsblk_json(&[(dev, serde_json::json!([s(&spaced), s(&root)]))]);
        assert_eq!(after(two).unwrap_err(), original);
        assert_eq!(after("not json".into()).unwrap_err(), original);
        assert_eq!(
            after(lsblk_json(&[("/dev/sdy1", serde_json::json!([s(&spaced)]))])).unwrap_err(),
            original
        );

        // a failed lookup does not mask the original error
        assert_eq!(
            mount_drive_with(dev, refused, &|_| Err("lsblk gone".into())).unwrap_err(),
            original
        );

        // failing to start the command stays an error and never consults the lookup
        let spawn = mount_drive_with(dev, &|_| Err("no udisksctl".into()), &|_| panic!("no lookup"));
        assert_eq!(spawn.unwrap_err(), "no udisksctl");
    }

    #[test]
    fn batch_rename_rejects_unselected_collision() {
        let dir = test_dir("collision");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "alpha").unwrap();
        fs::write(&b, "bravo").unwrap();

        let result = batch_rename(&[(a.to_string_lossy().to_string(), "b.txt".to_string())]);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(&b).unwrap(), "bravo");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recursive_search_finds_nested_names() {
        let dir = test_dir("search");
        let nested = dir.join("nested");
        fs::create_dir(&nested).unwrap();
        let hit = nested.join("needle.txt");
        let miss = nested.join("other.txt");
        fs::write(&hit, "alpha").unwrap();
        fs::write(&miss, "needle in content only").unwrap();

        let name_hits = search_dir(dir.to_str().unwrap(), "needle", false, false).unwrap();
        assert_eq!(name_hits.len(), 1);
        assert_eq!(name_hits[0].path, hit.to_string_lossy());

        let content_hits = search_dir(dir.to_str().unwrap(), "content only", true, false).unwrap();
        assert_eq!(content_hits.len(), 1);
        assert_eq!(content_hits[0].path, miss.to_string_lossy());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn media_classification_matches_old_preview_surface() {
        assert!(is_audio_path("/tmp/song.flac"));
        assert!(is_audio_path("/tmp/call.OPUS"));
        assert!(is_video_path("/tmp/movie.webm"));
        assert!(is_video_path("/tmp/capture.MP4"));
        assert!(!is_audio_path("/tmp/readme.md"));
        assert!(!is_video_path("/tmp/photo.png"));
    }

    #[test]
    fn archive_classification_covers_common_compressed_formats() {
        for path in [
            "/tmp/a.zip",
            "/tmp/a.7z",
            "/tmp/a.rar",
            "/tmp/a.cbz",
            "/tmp/a.cbr",
            "/tmp/a.tar",
            "/tmp/a.tar.gz",
            "/tmp/a.tgz",
            "/tmp/a.tar.bz2",
            "/tmp/a.tar.xz",
            "/tmp/a.txz",
            "/tmp/a.tar.zst",
            "/tmp/a.tzst",
            "/tmp/a.gz",
            "/tmp/a.bz2",
            "/tmp/a.xz",
            "/tmp/a.zst",
            "/tmp/a.iso",
        ] {
            assert!(is_archive_path(path), "{path}");
        }
        assert!(!is_archive_path("/tmp/readme.txt"));
    }

    /// A scratch dir with src/note.txt = "NEW" and dest/note.txt = "OLD".
    /// Throwaway test data lives under ~/Projects/Scratch, never /tmp. Each case
    /// removes its own directory on drop (also when an assertion panics), then
    /// removes the shared root only if it is empty, so siblings are never touched.
    struct ScratchCase(std::path::PathBuf);

    impl std::ops::Deref for ScratchCase {
        type Target = std::path::PathBuf;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl AsRef<Path> for ScratchCase {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchCase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            if let Some(root) = self.0.parent() {
                let _ = fs::remove_dir(root); // fails unless empty
            }
        }
    }

    fn scratch_case(tag: &str) -> ScratchCase {
        let dir = scratch_root().join(format!(
            "{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        ScratchCase(dir)
    }

    fn paste_conflict_dir() -> ScratchCase {
        let dir = scratch_case("wf-paste");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("dest")).unwrap();
        fs::write(dir.join("src/note.txt"), b"NEW").unwrap();
        fs::write(dir.join("dest/note.txt"), b"OLD").unwrap();
        dir
    }

    #[test]
    fn drag_action_modifiers_and_device_default() {
        let d = scratch_case("wf-drag");
        fs::create_dir_all(d.join("dest")).unwrap();
        fs::write(d.join("a.txt"), "a").unwrap();
        fs::write(d.join("b.txt"), "b").unwrap();
        let dest = d.join("dest").to_string_lossy().to_string();
        let a = d.join("a.txt").to_string_lossy().to_string();
        let b = d.join("b.txt").to_string_lossy().to_string();
        let srcs = vec![a.clone(), b.clone()];
        assert!(!drag_is_copy(&srcs, &dest, false, false), "same device defaults to MOVE");
        assert!(drag_is_copy(&srcs, &dest, true, false), "Ctrl copies");
        assert!(!drag_is_copy(&srcs, &dest, false, true), "Shift moves");
        assert!(drag_is_copy(&srcs, &dest, true, true), "Ctrl wins over Shift");
        // unreadable source or destination: explicit conservative COPY
        assert!(drag_is_copy(&[d.join("missing").to_string_lossy().to_string()], &dest, false, false));
        assert!(drag_is_copy(&srcs, &d.join("nodir").to_string_lossy(), false, false));
        // /proc is a different device from a scratch dir
        assert!(drag_is_copy(&["/proc/self".to_string()], &dest, false, false), "different device copies");
        // a real move of both files, then the real undo
        let items = resolve_paste(&srcs, &dest, false, PasteConflict::KeepBoth).unwrap();
        assert_eq!(items.len(), 2);
        assert!(!Path::new(&a).exists() && !Path::new(&b).exists());
        assert!(d.join("dest/a.txt").exists() && d.join("dest/b.txt").exists());
        restore_moved_items(&items).unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"a");
        assert_eq!(fs::read(&b).unwrap(), b"b");
        assert!(!d.join("dest/a.txt").exists() && !d.join("dest/b.txt").exists());
    }

    #[test]
    fn clash_preflight_and_three_choices() {
        let d = paste_conflict_dir();
        let src = d.join("src/note.txt").to_string_lossy().to_string();
        let dest = d.join("dest").to_string_lossy().to_string();
        fs::write(d.join("src/free.txt"), b"F").unwrap();
        let free = d.join("src/free.txt").to_string_lossy().to_string();
        // one clash plus one clean item, for copy and for move
        for is_copy in [true, false] {
            assert_eq!(check_conflicts(&[src.clone(), free.clone()], &dest, is_copy), vec!["note.txt"]);
        }
        // moving onto its own folder is a no-op, not a clash; copying is a clash
        let own = d.join("dest/note.txt").to_string_lossy().to_string();
        assert!(check_conflicts(&[own.clone()], &dest, false).is_empty());
        assert_eq!(check_conflicts(&[own], &dest, true), vec!["note.txt"]);
        // Cancel and Escape start nothing; the other two map to their modes
        assert_eq!(conflict_response("cancel"), None);
        assert_eq!(conflict_response("close"), None);
        assert_eq!(conflict_response("duplicate"), Some(PasteConflict::KeepBoth));
        assert_eq!(conflict_response("replace"), Some(PasteConflict::Replace));
        // Duplicate numbers from (2), then (3), keeping the extension; folders too
        let one = resolve_paste(&[src.clone()], &dest, true, PasteConflict::KeepBoth).unwrap();
        assert!(one[0].to.ends_with("note (2).txt"));
        let two = resolve_paste(&[src.clone()], &dest, true, PasteConflict::KeepBoth).unwrap();
        assert!(two[0].to.ends_with("note (3).txt"));
        assert_eq!(fs::read(d.join("dest/note.txt")).unwrap(), b"OLD");
        fs::create_dir_all(d.join("src/box")).unwrap();
        fs::create_dir_all(d.join("dest/box")).unwrap();
        let bx = d.join("src/box").to_string_lossy().to_string();
        let dup = resolve_paste(&[bx], &dest, true, PasteConflict::KeepBoth).unwrap();
        assert!(dup[0].to.ends_with("box (2)"));
        // Replace on a move: only the named destination item changes, source is consumed
        let out = resolve_paste(&[src.clone()], &dest, false, PasteConflict::Replace).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(fs::read(d.join("dest/note.txt")).unwrap(), b"NEW");
        assert!(!Path::new(&src).exists());
        assert!(d.join("dest/note (2).txt").exists(), "other items untouched");
        // Cancel: nothing called, so every byte is as it was
        assert_eq!(fs::read(&free).unwrap(), b"F");
    }

    #[test]
    fn paste_keep_both_leaves_the_existing_file_untouched() {
        let dir = paste_conflict_dir();
        let src = dir.join("src/note.txt").to_string_lossy().to_string();
        let dest = dir.join("dest").to_string_lossy().to_string();
        let out = resolve_paste(&[src], &dest, true, PasteConflict::KeepBoth).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(fs::read(dir.join("dest/note.txt")).unwrap(), b"OLD");
        assert_ne!(out[0].to, dir.join("dest/note.txt").to_string_lossy());
        assert_eq!(fs::read(&out[0].to).unwrap(), b"NEW");
    }

    #[test]
    fn paste_replace_overwrites_the_existing_file() {
        let dir = paste_conflict_dir();
        let src = dir.join("src/note.txt").to_string_lossy().to_string();
        let dest = dir.join("dest").to_string_lossy().to_string();
        let out = resolve_paste(&[src], &dest, true, PasteConflict::Replace).unwrap();
        assert_eq!(out.len(), 1);
        // The destination now holds the new bytes, at the ORIGINAL name.
        assert_eq!(fs::read(dir.join("dest/note.txt")).unwrap(), b"NEW");
        assert_eq!(out[0].to, dir.join("dest/note.txt").to_string_lossy());
        // And no "note (2).txt" was left behind.
        let extras: Vec<_> = fs::read_dir(dir.join("dest"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(extras, vec!["note.txt".to_string()]);
    }

    #[test]
    fn paste_replace_swaps_a_whole_folder_without_leaving_stale_children() {
        let dir = paste_conflict_dir();
        fs::create_dir_all(dir.join("src/box")).unwrap();
        fs::write(dir.join("src/box/new.txt"), b"NEW").unwrap();
        fs::create_dir_all(dir.join("dest/box")).unwrap();
        fs::write(dir.join("dest/box/stale.txt"), b"STALE").unwrap();
        let src = dir.join("src/box").to_string_lossy().to_string();
        let dest = dir.join("dest").to_string_lossy().to_string();
        resolve_paste(&[src], &dest, true, PasteConflict::Replace).unwrap();
        assert!(dir.join("dest/box/new.txt").exists());
        assert!(
            !dir.join("dest/box/stale.txt").exists(),
            "replacing a folder must not leave the old contents behind"
        );
    }

    #[test]
    fn appimage_and_friends_offer_extraction() {
        // The one that started this: an AppImage is an ELF with a squashfs
        // stapled on, so it never looked like an archive, and the user was left
        // with no way to extract it.
        assert!(is_archive_path("/home/z/Downloads/CodeOp-linux-4.0.0-beta+2 (1).AppImage"));
        assert!(is_archive_path("/tmp/Thing-x86_64.appimage"), "case-insensitive");
        for path in [
            "/tmp/pkg.deb",
            "/tmp/pkg.rpm",
            "/tmp/lib.jar",
            "/tmp/app.apk",
            "/tmp/mod.whl",
            "/tmp/setup.msi",
            "/tmp/boot.wim",
            "/tmp/root.squashfs",
        ] {
            assert!(is_archive_path(path), "{path} should be extractable");
        }
        // Still must not offer extraction for ordinary files.
        for path in ["/tmp/readme.txt", "/tmp/photo.png", "/tmp/clip.mp4", "/tmp/script.sh"] {
            assert!(!is_archive_path(path), "{path} is not an archive");
        }
        // And the extracted folder gets a sensible name.
        assert_eq!(archive_stem("/tmp/CodeOp-linux-4.0.0-beta+2 (1).AppImage"), "CodeOp-linux-4.0.0-beta+2 (1)");
    }

    #[test]
    fn archive_stem_strips_compound_suffixes() {
        assert_eq!(archive_stem("/tmp/backup.tar.gz"), "backup");
        assert_eq!(archive_stem("/tmp/backup.tar.zst"), "backup");
        assert_eq!(archive_stem("/tmp/photos.zip"), "photos");
        assert_eq!(archive_stem("/tmp/disc.iso"), "disc");
        // A dot inside the name is not a suffix boundary we should eat twice.
        assert_eq!(archive_stem("/tmp/v1.2.3.tar.xz"), "v1.2.3");
        assert_eq!(archive_stem("/tmp/noext"), "noext");
    }

    #[test]
    fn extract_into_new_dir_contains_the_files_and_never_overwrites() {
        let dir = scratch_case("wf-extract");
        // compress_zip runs `zip` FROM dest_dir with bare file names, so the
        // sources have to sit in the folder it is told to write into.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("out")).unwrap();
        fs::write(dir.join("src/a.txt"), b"A").unwrap();
        fs::write(dir.join("src/b.txt"), b"B").unwrap();
        let zip = compress_zip(
            &[
                dir.join("src/a.txt").to_string_lossy().to_string(),
                dir.join("src/b.txt").to_string_lossy().to_string(),
            ],
            &dir.join("src").to_string_lossy(),
        )
        .unwrap();

        let dest = dir.join("out").to_string_lossy().to_string();
        let first = extract_archive_into_new_dir(&zip, &dest).unwrap();
        assert!(Path::new(&first).join("a.txt").exists(), "files land inside the new folder");
        assert!(Path::new(&first).join("b.txt").exists());
        assert!(
            !Path::new(&dest).join("a.txt").exists(),
            "nothing may be scattered into the folder we were looking at"
        );

        // Extracting the same archive again must not overwrite the first result.
        let second = extract_archive_into_new_dir(&zip, &dest).unwrap();
        assert_ne!(first, second, "second extraction gets its own folder");
        assert!(Path::new(&second).join("a.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compress_zip_and_extract_archive_round_trip() {
        let dir = test_dir("archive");
        let source = dir.join("source.txt");
        let extract_dir = dir.join("extract");
        fs::write(&source, "archive payload").unwrap();
        fs::create_dir(&extract_dir).unwrap();

        let zip = compress_zip(
            &[source.to_string_lossy().to_string()],
            dir.to_str().unwrap(),
        )
        .unwrap();
        assert!(Path::new(&zip).exists());

        extract_archive(&zip, extract_dir.to_str().unwrap()).unwrap();
        let extracted = extract_dir.join("source.txt");
        assert_eq!(fs::read_to_string(extracted).unwrap(), "archive payload");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn window_registry_keeps_windows_apart_d19() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        struct S(&'static str);
        let dropped = Rc::new(Cell::new(0));
        struct Hold(Rc<Cell<i32>>);
        impl Drop for Hold {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let (a, b) = (Rc::new(RefCell::new(S("a"))), Rc::new(RefCell::new(S("b"))));
        let mut reg: WindowRegistry<S, &'static str, Hold> = WindowRegistry::default();
        reg.register(&a, "ui-a", Hold(dropped.clone()));
        reg.register(&b, "ui-b", Hold(dropped.clone()));
        // no aliasing: each state finds only its own ui and back
        assert_eq!(reg.ui_for_state(&a), Some("ui-a"));
        assert_eq!(reg.ui_for_state(&b), Some("ui-b"));
        assert!(Rc::ptr_eq(&reg.state_for_ui(|u| *u == "ui-a").unwrap(), &a));
        assert_eq!(reg.state_for_ui(|u| *u == "ui-b").unwrap().borrow().0, "b");
        // active: the active one, else the newest
        assert_eq!(reg.active(|u| *u == "ui-a").unwrap().1, "ui-a");
        assert_eq!(reg.active(|_| false).unwrap().1, "ui-b");
        // closing a releases only a's hold; b stays registered and alive
        drop(reg.unregister(&a));
        assert_eq!((reg.len(), dropped.get()), (1, 1));
        assert_eq!(reg.ui_for_state(&a), None);
        assert_eq!(reg.ui_for_state(&b), Some("ui-b"));
        assert!(reg.unregister(&a).is_none(), "closing twice releases nothing more");
        assert_eq!(dropped.get(), 1);
        // last close empties it
        drop(reg.unregister(&b));
        assert_eq!((reg.len(), dropped.get()), (0, 2));
        assert!(reg.active(|_| true).is_none());
    }

    #[test]
    fn guide_names_registry_is_exact() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/guide-names.json");
        let raw = fs::read_to_string(&path).expect("guide-names.json exists");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        let obj = v.as_object().expect("top level is an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "drive-open-crucial",
                "drive-open-stick",
                "drive-remount-crucial",
                "drive-remount-stick",
                "drive-unmount-crucial",
                "drive-unmount-stick",
                "item-menu-cap-d56",
                "item-menu-full-d60",
                "open-with-other-app",
                "terminal-here-folder",
                "trash-and-restore-file",
            ]
        );
        for (name, entry) in obj {
            let fields = entry.as_object().expect("each entry is an object");
            let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
            names.sort_unstable();
            // D-64: every entry carries `mark`, exactly `circle` or `arrow`; none is optional.
            assert_eq!(names, ["kind", "mark", "where"], "{name}");
            assert!(matches!(fields["mark"].as_str(), Some("circle" | "arrow")), "{name}: mark is circle or arrow");
            for f in fields.values() {
                assert!(f.as_str().is_some_and(|x| !x.is_empty()), "{name}");
            }
        }
        let mut circles: Vec<&str> =
            obj.iter().filter(|(_, e)| e["mark"].as_str() == Some("circle")).map(|(k, _)| k.as_str()).collect();
        circles.sort_unstable();
        assert_eq!(circles, ["terminal-here-folder", "trash-and-restore-file"], "the compact toolbar entries choose circle");
        let mut arrows: Vec<&str> =
            obj.iter().filter(|(_, e)| e["mark"].as_str() == Some("arrow")).map(|(k, _)| k.as_str()).collect();
        arrows.sort_unstable();
        assert_eq!(
            arrows,
            [
                "drive-open-crucial",
                "drive-open-stick",
                "drive-remount-crucial",
                "drive-remount-stick",
                "drive-unmount-crucial",
                "drive-unmount-stick",
                "item-menu-cap-d56",
                "item-menu-full-d60",
                "open-with-other-app",
            ],
            "every row entry chooses arrow"
        );
        assert!(!obj.contains_key("take-me-back-start"));
    }

    #[test]
    fn drive_space_uses_available_bytes_clamped_d46() {
        // f_bavail (250 blocks) is the value; there is no free-block input at all.
        let space = space_from_blocks(4096, 1000, 250).unwrap();
        assert_eq!((space.total, space.available), (4_096_000, 1_024_000));
        let clamped = space_from_blocks(4096, 1000, 5000).unwrap();
        assert_eq!(clamped.available, clamped.total);
        assert_eq!(space_from_blocks(4096, 0, 0), None);
        assert_eq!(space_from_blocks(0, 1000, 10), None);
        assert_eq!(space_from_blocks(u64::MAX, 2, 1).map(|s| s.available <= s.total), Some(true));
    }

    #[test]
    fn drive_space_words_and_fraction_agree_d46() {
        let gb: u64 = 1024 * 1024 * 1024;
        let view = space_view(Some(Space { total: 100 * gb, available: 25 * gb }));
        assert_eq!(view.words, "25 GB available");
        assert_eq!(view.fraction, Some(0.75));
        let over = space_view(Some(Space { total: 100 * gb, available: 900 * gb }));
        assert_eq!(over.fraction, Some(0.0));
        assert_eq!(over.words, "100 GB available");
        let full = space_view(Some(Space { total: 100 * gb, available: 0 }));
        assert_eq!((full.words.as_str(), full.fraction), ("0 B available", Some(1.0)));
        for none in [None, Some(Space { total: 0, available: 0 })] {
            let view = space_view(none);
            assert_eq!((view.words.as_str(), view.fraction), ("Space unavailable", None));
        }
        let mut drive = Drive {
            name: "d".into(),
            path: "/dev/x".into(),
            mountpoint: String::new(),
            size: 5,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable: false,
            space: Some(Space { total: 100 * gb, available: 25 * gb }),
        };
        let unmounted = drive.space_view();
        assert_eq!((unmounted.words.as_str(), unmounted.fraction), ("Space unavailable", None));
        drive.mountpoint = "/mnt/x".into();
        assert_eq!(drive.space_view().fraction, Some(0.75));
    }

    #[test]
    fn drive_scan_probes_only_mounted_drives_with_the_injected_probe_d46() {
        let probed = std::cell::RefCell::new(Vec::new());
        let drives = list_drives_with(&|mountpoint| {
            probed.borrow_mut().push(mountpoint.to_string());
            Some(Space { total: 1000, available: 1 })
        });
        let mounted: Vec<_> = drives.iter().filter(|d| !d.mountpoint.is_empty()).collect();
        assert_eq!(probed.borrow().len(), mounted.len(), "one probe per mounted drive");
        for d in &drives {
            let expected = (!d.mountpoint.is_empty()).then_some(Space { total: 1000, available: 1 });
            assert_eq!(d.space, expected, "{}", d.name);
        }
        let failed = list_drives_with(&|_| None);
        assert!(failed.iter().all(|d| d.space.is_none()), "a failed probe leaves no space");
    }
}
