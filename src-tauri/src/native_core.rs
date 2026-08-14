use adw::gtk::gio;
use adw::gtk::gio::prelude::FileExt;
use adw::gtk::glib;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

fn percent_encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
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
                });
            }
        }
    }

    out
}

pub fn mount_drive(device: &str) -> Result<String, String> {
    let output = Command::new("udisksctl")
        .args(["mount", "-b", device])
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .rsplit(" at ")
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches('.')
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
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

pub fn open_path(path: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
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

pub fn open_with(app_id: &str, path: &str) -> Result<(), String> {
    for dir in app_dirs() {
        let desktop_file = format!("{dir}/{app_id}");
        if Path::new(&desktop_file).exists() {
            return Command::new("gio")
                .args(["launch", &desktop_file, path])
                .spawn()
                .map(|_| ())
                .map_err(|err| err.to_string());
        }
    }
    Err("app not found".into())
}

pub fn open_terminal(path: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = &[
        ("konsole", &["--workdir"]),
        ("kgx", &["--working-directory"]),
        ("gnome-terminal", &["--working-directory"]),
        ("x-terminal-emulator", &[]),
        ("xterm", &[]),
    ];

    for (cmd, args) in candidates {
        let mut command = Command::new(cmd);
        if args.is_empty() {
            command.current_dir(path);
        } else {
            command.args(*args).arg(path);
        }
        match command.spawn() {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.to_string()),
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

pub fn make_dir(parent: &str, name: &str) -> Result<String, String> {
    validate_name(name)?;
    let path = Path::new(parent).join(name);
    fs::create_dir(&path).map_err(|err| err.to_string())?;
    Ok(path.to_string_lossy().to_string())
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
    fs::rename(path, &dest).map_err(|err| err.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

pub fn batch_rename(items: &[(String, String)]) -> Result<Vec<ResolvedItem>, String> {
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
        let dest_key = dest.to_string_lossy().to_string();
        if !destinations.insert(dest_key.clone()) {
            return Err(format!("duplicate target name: {new_name}"));
        }
        moving_from.insert(from.to_string_lossy().to_string());
        operations.push((from, dest));
    }

    if operations.is_empty() {
        return Ok(vec![]);
    }

    for (from, dest) in &operations {
        if !from.exists() {
            return Err(format!("missing source: {}", from.to_string_lossy()));
        }
        if dest.exists() && !moving_from.contains(&dest.to_string_lossy().to_string()) {
            return Err(format!("target already exists: {}", dest.to_string_lossy()));
        }
    }

    let pid = std::process::id();
    let mut staged = Vec::new();
    for (index, (from, _)) in operations.iter().enumerate() {
        let parent = from.parent().ok_or("no parent")?;
        let mut temp = parent.join(format!(".wayfinder-rename-{pid}-{index}.tmp"));
        let mut suffix = 0;
        while temp.exists() {
            suffix += 1;
            temp = parent.join(format!(".wayfinder-rename-{pid}-{index}-{suffix}.tmp"));
        }
        fs::rename(from, &temp).map_err(|err| err.to_string())?;
        staged.push(temp);
    }

    let mut done = Vec::new();
    for ((from, dest), temp) in operations.into_iter().zip(staged.into_iter()) {
        if let Err(err) = fs::rename(&temp, &dest) {
            let _ = fs::rename(&temp, &from);
            return Err(err.to_string());
        }
        done.push(ResolvedItem {
            from: from.to_string_lossy().to_string(),
            to: dest.to_string_lossy().to_string(),
        });
    }
    Ok(done)
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
    let info_dir = format!("{}/.local/share/Trash/info", home);

    for original in paths {
        if let Some(idx) = original.find("/Trash/files/") {
            let name = &original[idx + "/Trash/files/".len()..];
            if !name.is_empty() && !name.contains('/') {
                let uri = format!("trash:///{}", percent_encode_segment(name));
                run_cmd("gio", &["trash", "--restore", &uri])?;
                continue;
            }
        }

        let mut best: Option<(String, String)> = None;
        if let Ok(entries) = fs::read_dir(&info_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("trashinfo") {
                    continue;
                }
                let content = match fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(_) => continue,
                };

                let mut original_path = None;
                let mut deletion_date = String::new();
                for line in content.lines() {
                    if let Some(value) = line.strip_prefix("Path=") {
                        original_path = Some(percent_decode(value.trim()));
                    } else if let Some(value) = line.strip_prefix("DeletionDate=") {
                        deletion_date = value.trim().to_string();
                    }
                }

                if original_path.as_deref() == Some(original.as_str()) {
                    let name = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("")
                        .to_string();
                    if best
                        .as_ref()
                        .map(|(_, date)| deletion_date > *date)
                        .unwrap_or(true)
                    {
                        best = Some((name, deletion_date));
                    }
                }
            }
        }

        match best {
            Some((name, _)) => {
                let uri = format!("trash:///{}", percent_encode_segment(&name));
                run_cmd("gio", &["trash", "--restore", &uri])?;
            }
            None => return Err(format!("not found in trash: {original}")),
        }
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

pub fn check_conflicts(srcs: &[String], dest_dir: &str) -> Vec<String> {
    srcs.iter()
        .filter_map(|src| {
            let name = Path::new(src).file_name()?.to_str()?.to_string();
            Path::new(dest_dir).join(&name).exists().then_some(name)
        })
        .collect()
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
    /// Leave the existing item alone and do not paste that one.
    Skip,
}

pub fn resolve_paste(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
) -> Result<Vec<ResolvedItem>, String> {
    if !Path::new(dest_dir).is_dir() {
        return Err(format!("not a folder: {dest_dir}"));
    }

    let mut out = Vec::new();
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

        let dest = if target.exists() {
            match on_conflict {
                PasteConflict::KeepBoth => unique_target(dest_dir, name),
                PasteConflict::Skip => continue,
                PasteConflict::Replace => {
                    // Pasting a folder onto an existing folder cannot merge with
                    // `cp -T`, and overwriting in place would leave stale
                    // children behind, so clear the target first.
                    if same_path {
                        continue;
                    }
                    if target.is_dir() {
                        fs::remove_dir_all(&target).map_err(|err| err.to_string())?;
                    } else {
                        fs::remove_file(&target).map_err(|err| err.to_string())?;
                    }
                    target
                }
            }
        } else {
            target
        };
        let dest_str = dest.to_string_lossy().to_string();

        if is_copy {
            run_cmd_capture("cp", &["-a", "-T", "--", src, &dest_str])?;
        } else {
            run_cmd_capture("mv", &["-T", "--", src, &dest_str])?;
        }

        out.push(ResolvedItem {
            from: src.clone(),
            to: dest_str,
        });
    }
    Ok(out)
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
    let Ok(now) = glib::DateTime::now_local() else {
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
    if name.trim().is_empty() || name.contains('/') {
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
    let mut n = 1;
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

fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), String> {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| err.to_string())
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("{cmd} failed"))
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_unix() -> i64 {
        glib::DateTime::now_local().unwrap().to_unix()
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
        assert_eq!(fmt_modified(now_unix() - 10 * 60), "10 min ago");
        // 3h ago is "3 hr ago" only while it is still the same local day.
        let three_hours = now_unix() - 3 * 3600;
        let dt = glib::DateTime::from_unix_local(three_hours).unwrap();
        let now = glib::DateTime::now_local().unwrap();
        if same_day(&dt, &now) {
            assert_eq!(fmt_modified(three_hours), "3 hr ago");
        }
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

    fn test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "wayfinder-native-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
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
    fn paste_conflict_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wf-paste-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("dest")).unwrap();
        fs::write(dir.join("src/note.txt"), b"NEW").unwrap();
        fs::write(dir.join("dest/note.txt"), b"OLD").unwrap();
        dir
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
        let _ = fs::remove_dir_all(&dir);
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
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn paste_skip_changes_nothing() {
        let dir = paste_conflict_dir();
        let src = dir.join("src/note.txt").to_string_lossy().to_string();
        let dest = dir.join("dest").to_string_lossy().to_string();
        let out = resolve_paste(&[src], &dest, true, PasteConflict::Skip).unwrap();
        assert!(out.is_empty(), "skip must paste nothing");
        assert_eq!(fs::read(dir.join("dest/note.txt")).unwrap(), b"OLD");
        assert_eq!(fs::read(dir.join("src/note.txt")).unwrap(), b"NEW", "source survives");
        let _ = fs::remove_dir_all(&dir);
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
}
