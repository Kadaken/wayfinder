use serde::Serialize;
use std::fs;
use std::path::Path;

const IMG_EXT: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "svg", "ico", "avif", "heic",
];

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMG_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn icon_name(p: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "aiff" | "aif"
        | "alac" => "audio-x-generic",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "wmv" | "m4v" | "mpg" | "mpeg" | "3gp" | "flv"
        | "ts" => "video-x-generic",
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "svg" | "ico" | "avif"
        | "heic" => "image-x-generic",
        "pdf" => "application-pdf",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz" | "lzma" | "cab" => {
            "application-x-archive"
        }
        "iso" | "img" | "dmg" => "application-x-cd-image",
        "txt" | "log" | "md" | "rst" | "nfo" | "text" => "text-x-generic",
        "doc" | "docx" | "odt" | "rtf" => "x-office-document",
        "xls" | "xlsx" | "ods" | "csv" => "x-office-spreadsheet",
        "ppt" | "pptx" | "odp" => "x-office-presentation",
        "html" | "htm" => "text-html",
        "css" => "text-css",
        "json" => "application-json",
        "xml" => "text-xml",
        "sh" | "bash" | "zsh" | "fish" => "text-x-script",
        "py" => "text-x-python",
        "js" | "mjs" | "cjs" | "jsx" => "application-javascript",
        "ts2" | "tsx" => "application-javascript",
        "rs" => "text-rust",
        "c" | "h" => "text-x-csrc",
        "cpp" | "cc" | "cxx" | "hpp" => "text-x-c++src",
        "java" => "text-x-java",
        "go" => "text-x-go",
        "ttf" | "otf" | "woff" | "woff2" => "font-x-generic",
        "appimage" | "bin" | "run" | "exe" | "msi" => "application-x-executable",
        "rpm" => "application-x-rpm",
        "deb" => "application-x-deb",
        _ => "application-octet-stream",
    }
}

#[derive(Serialize)]
struct Entry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: i64,
    is_image: bool,
    icon: String,
}

#[tauri::command]
fn list_dir(path: String, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let rd = fs::read_dir(&path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let md = match ent.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let p = ent.path();
        let modified = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir: md.is_dir(),
            size: md.len(),
            modified,
            is_image: is_image(&p),
            icon: icon_name(&p, md.is_dir()).to_string(),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[tauri::command]
fn standard_dirs() -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let mut v = vec![("Home".to_string(), home.clone())];
    for d in ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"] {
        let p = format!("{}/{}", home, d);
        if Path::new(&p).is_dir() {
            v.push((d.to_string(), p));
        }
    }
    v.push(("Root".to_string(), "/".to_string()));
    let trash = format!("{}/.local/share/Trash/files", home);
    if Path::new(&trash).is_dir() {
        v.push(("Trash".to_string(), trash));
    }
    v
}

#[tauri::command]
fn parent_dir(path: String) -> String {
    Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        s.push(T[(n >> 18 & 63) as usize] as char);
        s.push(T[(n >> 12 & 63) as usize] as char);
        s.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        s.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    s
}

#[tauri::command]
fn read_data_url(path: String) -> Result<String, String> {
    let md = fs::metadata(&path).map_err(|e| e.to_string())?;
    if md.len() > 60 * 1024 * 1024 {
        return Err("too large to preview".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let ext = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    };
    Ok(format!("data:{};base64,{}", mime, b64(&bytes)))
}

#[derive(Serialize)]
struct Drive {
    name: String,
    path: String,        // device path or mountpoint for fuse
    mountpoint: String,  // "" if not mounted
    size: u64,
    fstype: String,
    kind: String,        // "drive" | "gdrive" | "network"
}

#[tauri::command]
fn list_drives() -> Vec<Drive> {
    let mut out = Vec::new();

    // Block devices via lsblk JSON
    if let Ok(o) = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT,PATH,TYPE"])
        .output()
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
            fn walk(node: &serde_json::Value, out: &mut Vec<Drive>) {
                let dev = node;
                let fstype = dev["fstype"].as_str().unwrap_or("");
                let typ = dev["type"].as_str().unwrap_or("");
                let mp = dev["mountpoint"].as_str().unwrap_or("");
                // skip uninteresting: no fs, swap, the EFI/boot, crypto containers
                let boring = fstype.is_empty()
                    || fstype == "swap"
                    || mp == "/boot"
                    || mp == "/boot/efi"
                    || fstype == "crypto_LUKS";
                if (typ == "part" || typ == "disk" || typ == "crypt") && !boring && mp != "/" {
                    let label = dev["label"].as_str().filter(|s| !s.is_empty());
                    let name = dev["name"].as_str().unwrap_or("?");
                    out.push(Drive {
                        name: label.map(|s| s.to_string()).unwrap_or_else(|| name.to_string()),
                        path: dev["path"].as_str().unwrap_or("").to_string(),
                        mountpoint: mp.to_string(),
                        size: dev["size"].as_u64().unwrap_or(0),
                        fstype: fstype.to_string(),
                        kind: "drive".into(),
                    });
                }
                if let Some(children) = dev["children"].as_array() {
                    for c in children {
                        walk(c, out);
                    }
                }
            }
            if let Some(devs) = v["blockdevices"].as_array() {
                for d in devs {
                    walk(d, &mut out);
                }
            }
        }
    }

    // Google Drive (rclone) + other fuse/network mounts from /proc/mounts
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 3 {
                continue;
            }
            let (dev, mp, fstype) = (f[0], f[1], f[2]);
            if mp.contains("/GoogleDrive") || fstype.contains("rclone") {
                out.push(Drive {
                    name: "Google Drive".into(),
                    path: mp.to_string(),
                    mountpoint: mp.to_string(),
                    size: 0,
                    fstype: "rclone".into(),
                    kind: "gdrive".into(),
                });
            } else if (fstype.starts_with("nfs") || fstype == "cifs" || fstype.starts_with("fuse."))
                && mp.starts_with("/home")
                && !mp.contains("/doc")
            {
                out.push(Drive {
                    name: mp.rsplit('/').next().unwrap_or(dev).to_string(),
                    path: mp.to_string(),
                    mountpoint: mp.to_string(),
                    size: 0,
                    fstype: fstype.to_string(),
                    kind: "network".into(),
                });
            }
        }
    }
    out
}

#[tauri::command]
fn mount_drive(device: String) -> Result<String, String> {
    let o = std::process::Command::new("udisksctl")
        .args(["mount", "-b", &device])
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        // output: "Mounted /dev/sdb1 at /run/media/USER/LABEL"
        let s = String::from_utf8_lossy(&o.stdout);
        Ok(s.rsplit(" at ").next().unwrap_or("").trim().trim_end_matches('.').to_string())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).to_string())
    }
}

fn run_cmd(prog: &str, args: &[&str]) -> Result<(), String> {
    let o = std::process::Command::new(prog)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).to_string())
    }
}

#[tauri::command]
fn copy_paths(srcs: Vec<String>, dest_dir: String) -> Result<(), String> {
    let dest_slash = format!("{}/", dest_dir.trim_end_matches('/'));
    for s in &srcs {
        let o = std::process::Command::new("cp")
            .args(["-a", "--", s])
            .arg(&dest_slash)
            .output()
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).to_string());
        }
    }
    Ok(())
}

#[tauri::command]
fn move_paths(srcs: Vec<String>, dest_dir: String) -> Result<(), String> {
    let dest_slash = format!("{}/", dest_dir.trim_end_matches('/'));
    for s in &srcs {
        let o = std::process::Command::new("mv")
            .args(["--", s])
            .arg(&dest_slash)
            .output()
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).to_string());
        }
    }
    Ok(())
}

#[tauri::command]
fn delete_paths(paths: Vec<String>) -> Result<(), String> {
    // send to trash via gio (integrates with the XDG/KDE trash)
    for p in &paths {
        run_cmd("gio", &["trash", "--", p])?;
    }
    Ok(())
}

#[tauri::command]
fn rename_path(path: String, new_name: String) -> Result<(), String> {
    if new_name.contains('/') || new_name.is_empty() {
        return Err("invalid name".into());
    }
    let parent = Path::new(&path).parent().ok_or("no parent")?;
    let dest = parent.join(&new_name);
    fs::rename(&path, &dest).map_err(|e| e.to_string())
}

#[tauri::command]
fn make_dir(parent: String, name: String) -> Result<String, String> {
    if name.contains('/') || name.is_empty() {
        return Err("invalid name".into());
    }
    let p = Path::new(&parent).join(&name);
    fs::create_dir(&p).map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().to_string())
}

#[derive(Serialize)]
struct AppEntry {
    id: String,
    name: String,
}

fn app_dirs() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        format!("{}/.local/share/applications", home),
        "/usr/local/share/applications".to_string(),
        "/usr/share/applications".to_string(),
        "/var/lib/flatpak/exports/share/applications".to_string(),
        format!("{}/.local/share/flatpak/exports/share/applications", home),
    ]
}

#[tauri::command]
fn open_with_apps(path: String) -> Vec<AppEntry> {
    let mime = std::process::Command::new("xdg-mime")
        .args(["query", "filetype", &path])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if mime.is_empty() {
        return vec![];
    }
    let dirs = app_dirs();
    let mut ids: Vec<String> = Vec::new();
    for d in &dirs {
        if let Ok(txt) = fs::read_to_string(format!("{}/mimeinfo.cache", d)) {
            for line in txt.lines() {
                if let Some((m, apps)) = line.split_once('=') {
                    if m == mime {
                        for a in apps.split(';') {
                            if !a.is_empty() && !ids.contains(&a.to_string()) {
                                ids.push(a.to_string());
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
        let mut nodisplay = false;
        for d in &dirs {
            if let Ok(txt) = fs::read_to_string(format!("{}/{}", d, id)) {
                for line in txt.lines() {
                    if let Some(n) = line.strip_prefix("Name=") {
                        name = n.to_string();
                    }
                    if line == "NoDisplay=true" || line == "Terminal=true" {
                        nodisplay = true;
                    }
                }
                break;
            }
        }
        if !nodisplay {
            out.push(AppEntry { id, name });
        }
    }
    out
}

#[tauri::command]
fn open_with(app_id: String, path: String) -> Result<(), String> {
    for d in app_dirs() {
        let p = format!("{}/{}", d, app_id);
        if Path::new(&p).exists() {
            return std::process::Command::new("gio")
                .args(["launch", &p, &path])
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
    }
    Err("app not found".into())
}

#[derive(Serialize)]
struct Props {
    name: String,
    path: String,
    kind: String,
    size: u64,
    is_dir: bool,
    modified: i64,
    created: i64,
    permissions: String,
    items: i64,
}

#[tauri::command]
fn properties(path: String) -> Result<Props, String> {
    use std::os::unix::fs::PermissionsExt;
    let md = fs::metadata(&path).map_err(|e| e.to_string())?;
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };
    let p = Path::new(&path);
    let items = if md.is_dir() {
        fs::read_dir(&path).map(|r| r.count() as i64).unwrap_or(-1)
    } else {
        -1
    };
    let kind = if md.is_dir() {
        "Folder".to_string()
    } else {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{} file", e.to_uppercase()))
            .unwrap_or_else(|| "File".to_string())
    };
    Ok(Props {
        name: p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.clone()),
        path: path.clone(),
        kind,
        size: md.len(),
        is_dir: md.is_dir(),
        modified: secs(md.modified()),
        created: secs(md.created()),
        permissions: format!("{:o}", md.permissions().mode() & 0o777),
        items,
    })
}

#[tauri::command]
fn delete_permanent(paths: Vec<String>) -> Result<(), String> {
    for p in &paths {
        let md = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
        if md.is_dir() {
            fs::remove_dir_all(p).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn new_window(path: String) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn start_path() -> Option<String> {
    std::env::args()
        .nth(1)
        .filter(|a| !a.starts_with('-') && Path::new(a).is_dir())
}

#[tauri::command]
fn icon_svg(name: String) -> Result<String, String> {
    let themes = ["breeze-dark", "breeze", "hicolor", "Adwaita"];
    let cats = ["mimetypes", "places", "apps", "devices", "status"];
    let sizes = ["64", "48", "32", "24", "22", "16", "scalable"];
    let mut names = vec![name.clone()];
    for fb in ["application-octet-stream", "text-x-generic", "unknown"] {
        names.push(fb.to_string());
    }
    for nm in &names {
        for th in &themes {
            for cat in &cats {
                for sz in &sizes {
                    let p = format!("/usr/share/icons/{}/{}/{}/{}.svg", th, cat, sz, nm);
                    if Path::new(&p).exists() {
                        if let Ok(data) = fs::read(&p) {
                            return Ok(format!("data:image/svg+xml;base64,{}", b64(&data)));
                        }
                    }
                }
            }
        }
    }
    Err(format!("icon not found: {}", name))
}

#[tauri::command]
fn open_terminal(path: String) -> Result<(), String> {
    std::process::Command::new("konsole")
        .args(["--workdir", &path])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_dir,
            standard_dirs,
            parent_dir,
            open_path,
            read_data_url,
            list_drives,
            mount_drive,
            copy_paths,
            move_paths,
            delete_paths,
            rename_path,
            make_dir,
            icon_svg,
            properties,
            delete_permanent,
            new_window,
            start_path,
            open_with_apps,
            open_with,
            open_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
