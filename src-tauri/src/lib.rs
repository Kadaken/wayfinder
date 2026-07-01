use serde::Serialize;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use tauri::Emitter;

// Percent-decode a URI path segment string (handles %XX byte escapes).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --- Local media HTTP server ---------------------------------------------
// WebKit2GTK's GStreamer media backend can't fetch from Tauri custom URI
// schemes, so HTML5 <video> on Linux stays black. We run a tiny localhost
// HTTP/1.1 server with Range support that GStreamer's souphttpsrc CAN read.
static MEDIA_PORT: OnceLock<u16> = OnceLock::new();
static MEDIA_TOKEN: OnceLock<String> = OnceLock::new();
// Only files explicitly registered via media_url() may be served. This stops the
// local server from being a read-anything-on-disk oracle even if the token leaks.
static MEDIA_ALLOW: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();

// 32 hex chars of cryptographically-strong randomness from the OS RNG.
fn random_token() -> String {
    let mut buf = [0u8; 16];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn start_media_server() {
    use std::net::TcpListener;
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(_) => return,
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(_) => return,
    };
    let token = random_token();
    let _ = MEDIA_ALLOW.set(std::sync::Mutex::new(std::collections::HashSet::new()));
    let _ = MEDIA_PORT.set(port);
    let _ = MEDIA_TOKEN.set(token);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = serve_media_conn(stream);
            });
        }
    });
}

fn serve_media_conn(mut stream: std::net::TcpStream) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // "GET /<token>/<percent-encoded-abs-path> HTTP/1.1"
    let target = request_line.split_whitespace().nth(1).unwrap_or("");

    // Read headers; capture Range.
    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Range:").or_else(|| trimmed.strip_prefix("range:")) {
            range_header = Some(v.trim().to_string());
        }
    }

    let write_status = |stream: &mut std::net::TcpStream, code: &str| -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            code
        )
    };

    // Validate token, then decode file path.
    let rest = target.trim_start_matches('/');
    let (token, encoded_path) = match rest.split_once('/') {
        Some(parts) => parts,
        None => return write_status(&mut stream, "400 Bad Request"),
    };
    if MEDIA_TOKEN.get().map(|t| t.as_str()) != Some(token) {
        return write_status(&mut stream, "403 Forbidden");
    }
    let path = percent_decode(&format!("/{}", encoded_path));
    // Only serve files the app explicitly opened for preview.
    let allowed = MEDIA_ALLOW
        .get()
        .and_then(|m| m.lock().ok().map(|s| s.contains(&path)))
        .unwrap_or(false);
    if !allowed {
        return write_status(&mut stream, "403 Forbidden");
    }
    let mime = guess_video_mime(&path);

    let mut file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return write_status(&mut stream, "404 Not Found"),
    };
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);

    // Parse "bytes=start-end".
    let range = range_header
        .as_deref()
        .and_then(|v| v.strip_prefix("bytes="))
        .map(|v| {
            let mut it = v.splitn(2, '-');
            let start = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let end = it
                .next()
                .and_then(|s| if s.is_empty() { None } else { s.parse::<u64>().ok() })
                .unwrap_or(total.saturating_sub(1));
            (start, end.min(total.saturating_sub(1)))
        });

    if let Some((start, end)) = range {
        let len = end.saturating_sub(start) + 1;
        file.seek(SeekFrom::Start(start))?;
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
            mime, len, start, end, total
        );
        stream.write_all(header.as_bytes())?;
        let mut remaining = len;
        let mut buf = vec![0u8; 64 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            if stream.write_all(&buf[..n]).is_err() {
                break; // client closed (seek/skip) — fine
            }
            remaining -= n as u64;
        }
    } else {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            mime, total
        );
        stream.write_all(header.as_bytes())?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if stream.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    }
    let _ = stream.flush();
    Ok(())
}

// Returns the localhost URL the <video> element should load, or an error if
// the media server didn't start.
#[tauri::command]
fn media_url(path: String) -> Result<String, String> {
    let port = MEDIA_PORT.get().ok_or("media server not running")?;
    let token = MEDIA_TOKEN.get().ok_or("media server not running")?;
    // Whitelist this exact path so the server will serve it.
    if let Some(allow) = MEDIA_ALLOW.get() {
        if let Ok(mut set) = allow.lock() {
            set.insert(path.clone());
        }
    }
    let encoded: String = path
        .split('/')
        .map(|seg| {
            seg.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                        (b as char).to_string()
                    } else {
                        format!("%{:02X}", b)
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/");
    // path begins with '/', so `encoded` does too -> no double slash after token.
    Ok(format!("http://127.0.0.1:{}/{}{}", port, token, encoded))
}

fn guess_video_mime(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" => "video/mp2t",
        "3gp" => "video/3gpp",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

#[derive(Clone, Serialize)]
struct PickerConfig {
    multiple: bool,
    directory: bool,
    starting_dir: Option<String>,
    filter: Option<String>,
}

#[derive(Clone)]
struct PickerState {
    config: PickerConfig,
    out_file: String,
}

static PICKER_STATE: OnceLock<Option<PickerState>> = OnceLock::new();

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
        "ts" | "tsx" => "application-javascript",
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

fn file_contains(p: &Path, q_lower: &str) -> bool {
    use std::io::Read;
    let md = match fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if md.len() > 5 * 1024 * 1024 {
        return false; // skip large files for content search
    }
    let f = match fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = Vec::new();
    if f.take(5 * 1024 * 1024).read_to_end(&mut buf).is_err() {
        return false;
    }
    if buf.contains(&0) {
        return false; // looks binary
    }
    String::from_utf8_lossy(&buf).to_lowercase().contains(q_lower)
}

// Recursively search under `root` for entries whose name (and optionally text
// content) contains `query`. Case-insensitive, capped to keep the UI responsive.
#[tauri::command]
fn search_dir(
    root: String,
    query: String,
    content: bool,
    show_hidden: bool,
) -> Result<Vec<Entry>, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }
    const MAX_RESULTS: usize = 2000;
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_RESULTS {
            break;
        }
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
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
            let is_dir = md.is_dir();
            // file_type() reflects the entry itself (not its target), so we can
            // tell a real directory from a symlink without an extra syscall.
            let is_symlink = ent.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            let mut matched = name.to_lowercase().contains(&q);
            if !matched && content && !is_dir {
                matched = file_contains(&p, &q);
            }
            if matched {
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.push(Entry {
                    name,
                    path: p.to_string_lossy().to_string(),
                    is_dir,
                    size: md.len(),
                    modified,
                    is_image: is_image(&p),
                    icon: icon_name(&p, is_dir).to_string(),
                });
                if out.len() >= MAX_RESULTS {
                    break;
                }
            }
            // Only descend into real directories; following symlinks here can
            // loop forever (e.g. a link back to an ancestor, or /proc self-refs).
            if is_dir && !is_symlink {
                stack.push(p);
            }
        }
    }
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
    if md.len() > 100 * 1024 * 1024 {
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
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "opus" => "audio/ogg",
        "wma" => "audio/x-ms-wma",
        "aiff" | "aif" => "audio/aiff",
        "alac" => "audio/mp4",
        _ => "application/octet-stream",
    };
    Ok(format!("data:{};base64,{}", mime, b64(&bytes)))
}

// Read a text file for the preview pane. Caps the read, rejects binaries (NUL byte),
// and lossily decodes UTF-8 so odd encodings still show something readable.
#[tauri::command]
fn read_text_preview(path: String) -> Result<String, String> {
    use std::io::Read;
    let md = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !md.is_file() {
        return Err("not a file".into());
    }
    const CAP: usize = 256 * 1024; // preview at most 256 KB
    let mut f = fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; CAP];
    // A single read() may return fewer bytes than requested even for a regular
    // file, so fill the buffer until it's full or the file ends.
    let mut n = 0;
    while n < CAP {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(e) => return Err(e.to_string()),
        }
    }
    buf.truncate(n);
    if buf.contains(&0) {
        return Err("binary".into()); // NUL byte => not a text file
    }
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if md.len() as usize > n {
        s.push_str("\n\n… (preview truncated — showing the first 256 KB)");
    }
    Ok(s)
}

#[derive(Serialize)]
struct Drive {
    name: String,
    path: String,        // device path or mountpoint for fuse
    mountpoint: String,  // "" if not mounted
    size: u64,
    fstype: String,
    kind: String,        // "drive" | "gdrive" | "network"
    removable: bool,     // true for USB / hot-pluggable devices (eligible for Eject)
}

#[tauri::command]
fn list_drives() -> Vec<Drive> {
    let mut out = Vec::new();

    // Block devices via lsblk JSON
    if let Ok(o) = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT,PATH,TYPE,RM"])
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
                    let removable = dev["rm"].as_bool().unwrap_or(false)
                        || dev["rm"].as_str() == Some("1");
                    out.push(Drive {
                        name: label.map(|s| s.to_string()).unwrap_or_else(|| name.to_string()),
                        path: dev["path"].as_str().unwrap_or("").to_string(),
                        mountpoint: mp.to_string(),
                        size: dev["size"].as_u64().unwrap_or(0),
                        fstype: fstype.to_string(),
                        kind: "drive".into(),
                        removable,
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
                    removable: false,
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
                    removable: false,
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

// Resolve the whole-disk device for a partition (/dev/sda1 -> /dev/sda); pass-through if already a disk.
fn whole_disk_of(device: &str) -> String {
    if let Ok(o) = std::process::Command::new("lsblk")
        .args(["-no", "PKNAME", device])
        .output()
    {
        let parent = String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !parent.is_empty() {
            return format!("/dev/{}", parent);
        }
    }
    device.to_string()
}

#[tauri::command]
fn unmount_drive(device: String) -> Result<(), String> {
    let o = std::process::Command::new("udisksctl")
        .args(["unmount", "-b", &device])
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

#[tauri::command]
fn eject_drive(device: String) -> Result<(), String> {
    let disk = whole_disk_of(&device);
    // Unmount every mounted partition on this disk before cutting power.
    if let Ok(o) = std::process::Command::new("lsblk")
        .args(["-nro", "PATH,MOUNTPOINT", &disk])
        .output()
    {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let mut it = line.split_whitespace();
            let part = it.next().unwrap_or("");
            let mp = it.next().unwrap_or("");
            if !part.is_empty() && !mp.is_empty() {
                let _ = std::process::Command::new("udisksctl")
                    .args(["unmount", "-b", part])
                    .output();
            }
        }
    }
    // Power off the whole device so it's safe to physically remove.
    let o = std::process::Command::new("udisksctl")
        .args(["power-off", "-b", &disk])
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
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

#[derive(Serialize)]
struct ResolvedItem {
    from: String,
    to: String,
}

// Return the basenames of srcs that already exist in dest_dir (paste/drop conflicts).
#[tauri::command]
fn check_conflicts(srcs: Vec<String>, dest_dir: String) -> Vec<String> {
    srcs.iter()
        .filter_map(|s| {
            let name = Path::new(s).file_name()?.to_str()?.to_string();
            if Path::new(&dest_dir).join(&name).exists() {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

// Find a non-colliding "name (1).ext" style target inside dest_dir.
fn unique_target(dest_dir: &str, name: &str) -> std::path::PathBuf {
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

// Copy or move srcs into dest_dir, resolving name conflicts by mode:
// "replace" overwrites, "skip" leaves the existing file, "keepboth" adds a suffix.
// Returns what ended up where so the caller can build an accurate undo entry.
#[tauri::command]
fn resolve_paste(
    srcs: Vec<String>,
    dest_dir: String,
    mode: String,
    is_copy: bool,
) -> Result<Vec<ResolvedItem>, String> {
    let mut out = Vec::new();
    for s in &srcs {
        let name = Path::new(s)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("bad name")?
            .to_string();
        let target = Path::new(&dest_dir).join(&name);
        // Pasting an item onto itself (same source and destination path): for
        // replace/move this would delete the only copy before the cp/mv runs,
        // losing the file. Skip it — the file is already where it belongs.
        if target.exists() && mode != "keepboth" {
            let same = matches!(
                (fs::canonicalize(s), fs::canonicalize(&target)),
                (Ok(a), Ok(b)) if a == b
            );
            if same {
                continue;
            }
        }
        let dest_path = if target.exists() {
            match mode.as_str() {
                "skip" => continue,
                "keepboth" => unique_target(&dest_dir, &name),
                _ => {
                    // replace: remove the existing target first for a clean swap
                    if target.is_dir() {
                        fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
                    } else {
                        fs::remove_file(&target).map_err(|e| e.to_string())?;
                    }
                    target.clone()
                }
            }
        } else {
            target.clone()
        };
        let dest_str = dest_path.to_string_lossy().to_string();
        let o = if is_copy {
            std::process::Command::new("cp")
                .args(["-a", "-T", "--", s, &dest_str])
                .output()
        } else {
            std::process::Command::new("mv")
                .args(["-T", "--", s, &dest_str])
                .output()
        }
        .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).to_string());
        }
        out.push(ResolvedItem {
            from: s.clone(),
            to: dest_str,
        });
    }
    Ok(out)
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
    if let Some(Some(state)) = PICKER_STATE.get() {
        if let Some(dir) = state.config.starting_dir.clone() {
            return Some(dir);
        }
    }
    let arg = std::env::args()
        .nth(1)
        .filter(|a| !a.starts_with('-'))?;
    let p = Path::new(&arg);
    if p.is_dir() {
        Some(arg)
    } else if p.is_file() {
        // launched with a file path (e.g. browser "Show in folder") — open the parent
        p.parent().map(|d| d.to_string_lossy().into_owned())
    } else {
        None
    }
}

// Returns the file to select after navigating (when launched with a file path arg)
#[tauri::command]
fn start_select() -> Option<String> {
    let arg = std::env::args()
        .nth(1)
        .filter(|a| !a.starts_with('-'))?;
    let p = Path::new(&arg);
    if p.is_file() {
        Some(arg)
    } else {
        None
    }
}

#[tauri::command]
fn picker_options() -> Option<PickerConfig> {
    PICKER_STATE
        .get()
        .and_then(|state| state.as_ref().map(|value| value.config.clone()))
}

#[tauri::command]
fn picker_confirm(paths: Vec<String>) -> Result<(), String> {
    let Some(Some(state)) = PICKER_STATE.get() else {
        return Err("picker mode is not active".into());
    };
    let mut output = String::new();
    for path in paths {
        output.push_str(path.trim());
        output.push('\n');
    }
    fs::write(&state.out_file, output).map_err(|e| e.to_string())?;
    std::process::exit(0);
}

#[tauri::command]
fn picker_cancel() -> Result<(), String> {
    if let Some(Some(state)) = PICKER_STATE.get() {
        let _ = fs::write(&state.out_file, "");
    }
    std::process::exit(1);
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

#[derive(Serialize)]
struct Dir {
    name: String,
    path: String,
    has_children: bool,
}

#[tauri::command]
fn list_subdirs(path: String, show_hidden: bool) -> Vec<Dir> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&path) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let p = ent.path();
            if p.is_dir() {
                let has = fs::read_dir(&p)
                    .map(|r| r.flatten().any(|e| e.path().is_dir()))
                    .unwrap_or(false);
                out.push(Dir {
                    name,
                    path: p.to_string_lossy().to_string(),
                    has_children: has,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[tauri::command]
fn compress_zip(paths: Vec<String>, dest_dir: String) -> Result<String, String> {
    if paths.is_empty() {
        return Err("nothing to compress".into());
    }
    let stem = Path::new(&paths[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Archive");
    let zip_name = if paths.len() == 1 {
        format!("{}.zip", stem)
    } else {
        "Archive.zip".to_string()
    };
    let zip_path = format!("{}/{}", dest_dir.trim_end_matches('/'), zip_name);
    let mut args = vec!["-r".to_string(), zip_path.clone()];
    for p in &paths {
        if let Some(name) = Path::new(p).file_name().and_then(|s| s.to_str()) {
            args.push(name.to_string());
        }
    }
    let o = std::process::Command::new("zip")
        .current_dir(&dest_dir)
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(zip_path)
    } else {
        Err(String::from_utf8_lossy(&o.stderr).to_string())
    }
}

#[tauri::command]
fn extract_archive(path: String, dest_dir: String) -> Result<(), String> {
    let l = path.to_lowercase();
    if l.ends_with(".zip") {
        run_cmd("unzip", &["-o", &path, "-d", &dest_dir])
    } else if l.ends_with(".tar.gz")
        || l.ends_with(".tgz")
        || l.ends_with(".tar.bz2")
        || l.ends_with(".tar.xz")
        || l.ends_with(".tar")
    {
        run_cmd("tar", &["-xf", &path, "-C", &dest_dir])
    } else if l.ends_with(".7z") {
        run_cmd("7z", &["x", &format!("-o{}", dest_dir), &path])
    } else {
        Err("unsupported archive type".into())
    }
}

#[tauri::command]
fn new_file(parent: String, name: String) -> Result<String, String> {
    if name.contains('/') || name.is_empty() {
        return Err("invalid name".into());
    }
    let p = Path::new(&parent).join(&name);
    if p.exists() {
        return Err("already exists".into());
    }
    fs::File::create(&p).map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
fn empty_trash() -> Result<(), String> {
    run_cmd("gio", &["trash", "--empty"])
}

fn percent_encode_segment(seg: &str) -> String {
    seg.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{:02X}", b)
            }
        })
        .collect()
}

// Restore trashed items back to their original locations (used by Undo and the
// Trash view). Each input is the ORIGINAL path the file had before deletion.
// We read the XDG trashinfo metadata to find the matching trash entry with the
// most recent deletion date, then restore it via gio.
#[tauri::command]
fn restore_from_trash(paths: Vec<String>) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let info_dir = format!("{}/.local/share/Trash/info", home);
    for orig in &paths {
        // Trash-view path (…/Trash/files/NAME): restore that exact entry by name.
        if let Some(idx) = orig.find("/Trash/files/") {
            let name = &orig[idx + "/Trash/files/".len()..];
            if !name.is_empty() && !name.contains('/') {
                let uri = format!("trash:///{}", percent_encode_segment(name));
                run_cmd("gio", &["trash", "--restore", &uri])?;
                continue;
            }
        }
        let mut best: Option<(String, String)> = None; // (trash name, deletion date)
        if let Ok(rd) = fs::read_dir(&info_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("trashinfo") {
                    continue;
                }
                let content = match fs::read_to_string(&p) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let mut orig_path = None;
                let mut del_date = String::new();
                for line in content.lines() {
                    if let Some(v) = line.strip_prefix("Path=") {
                        orig_path = Some(percent_decode(v.trim()));
                    } else if let Some(v) = line.strip_prefix("DeletionDate=") {
                        del_date = v.trim().to_string();
                    }
                }
                if orig_path.as_deref() == Some(orig.as_str()) {
                    let name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if best.as_ref().map(|(_, d)| del_date > *d).unwrap_or(true) {
                        best = Some((name, del_date));
                    }
                }
            }
        }
        match best {
            Some((name, _)) => {
                let uri = format!("trash:///{}", percent_encode_segment(&name));
                run_cmd("gio", &["trash", "--restore", &uri])?;
            }
            None => return Err(format!("not found in trash: {}", orig)),
        }
    }
    Ok(())
}

#[tauri::command]
fn open_terminal(path: String) -> Result<(), String> {
    std::process::Command::new("konsole")
        .args(["--workdir", &path])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn parse_picker_state() -> Option<PickerState> {
    let mut args = std::env::args().skip(1);
    let mut pick = false;
    let mut multiple = false;
    let mut directory = false;
    let mut starting_dir = None;
    let mut filter = None;
    let mut out_file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pick" => pick = true,
            "--multiple" => multiple = true,
            "--directory" => directory = true,
            "--out" => out_file = args.next(),
            "--starting-dir" => starting_dir = args.next(),
            "--filter" => filter = args.next(),
            _ => {}
        }
    }
    if !pick {
        return None;
    }
    let out_file = out_file?;
    Some(PickerState {
        config: PickerConfig {
            multiple,
            directory,
            starting_dir,
            filter,
        },
        out_file,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if std::env::args().any(|arg| arg == "--pick-capable") {
        std::process::exit(0);
    }
    let picker_state = parse_picker_state();
    let _ = PICKER_STATE.set(picker_state.clone());
    start_media_server();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut child = match std::process::Command::new("udevadm")
                    .args(["monitor", "--udev", "--subsystem-match=block"])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => return,
                };
                use std::io::BufRead;
                let stdout = match child.stdout.take() {
                    Some(s) => s,
                    None => return,
                };
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) if l.starts_with("UDEV") && l.contains("/devices/") => {
                            let _ = handle.emit("drives-changed", ());
                        }
                        Err(_) => break,
                        _ => {}
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            search_dir,
            standard_dirs,
            parent_dir,
            picker_options,
            picker_confirm,
            picker_cancel,
            open_path,
            media_url,
            read_data_url,
            read_text_preview,
            list_drives,
            mount_drive,
            unmount_drive,
            eject_drive,
            copy_paths,
            move_paths,
            check_conflicts,
            resolve_paste,
            delete_paths,
            rename_path,
            make_dir,
            icon_svg,
            properties,
            delete_permanent,
            new_window,
            start_path,
            start_select,
            open_with_apps,
            open_with,
            list_subdirs,
            compress_zip,
            extract_archive,
            new_file,
            empty_trash,
            restore_from_trash,
            open_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
