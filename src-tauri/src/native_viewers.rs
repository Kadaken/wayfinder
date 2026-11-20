// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Kadaken

//! D-362: the parts of the preview pane that need no GTK -- which documents
//! the pane can render, running the outside tools (pdftoppm, soffice, gs,
//! magick, ffmpeg) with cancellation and a time limit, the cache folders, the
//! SWF header, and the BMP header check. The GTK side lives in native.rs.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What kind of document the pane renders as pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    /// Already a PDF: pages are drawn directly.
    Pdf,
    /// LibreOffice converts it to a PDF first.
    Office,
    /// Ghostscript converts it to a PDF first.
    PostScript,
    /// LibreOffice converts it to a PDF first; the pane can also show source.
    Html,
}

/// Office, OpenDocument and other formats LibreOffice opens. `csv` stays a
/// text preview on purpose; `epub` is absent because LibreOffice cannot open
/// it here ("source file could not be loaded", checked 20 Nov 2026).
const OFFICE_EXT: [&str; 36] = [
    "doc", "docx", "dot", "dotx", "odt", "ott", "rtf", "wpd", "wps", "abw", "sxw", "stw",
    "xls", "xlsx", "xlt", "xltx", "ods", "ots", "sxc", "xlsm",
    "ppt", "pptx", "pot", "potx", "pps", "ppsx", "odp", "otp", "sxi",
    "odg", "otg", "odf", "pages", "key", "numbers", "vsd",
];
const PS_EXT: [&str; 2] = ["ps", "eps"];
const HTML_EXT: [&str; 2] = ["htm", "html"];

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

pub fn doc_kind(path: &str) -> Option<DocKind> {
    let ext = ext_of(path);
    let ext = ext.as_str();
    if ext == "pdf" {
        Some(DocKind::Pdf)
    } else if OFFICE_EXT.contains(&ext) {
        Some(DocKind::Office)
    } else if PS_EXT.contains(&ext) {
        Some(DocKind::PostScript)
    } else if HTML_EXT.contains(&ext) {
        Some(DocKind::Html)
    } else {
        None
    }
}

pub fn is_swf_path(path: &str) -> bool {
    ext_of(path) == "swf"
}

pub fn tool_in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join(name);
                p.is_file()
            })
        })
        .unwrap_or(false)
}

/// The sentence the pane shows when a tool a kind of preview needs is absent.
pub fn missing_tool_message(kind: DocKind) -> Option<String> {
    let need = |tools: &[&str], package: &str, what: &str| -> Option<String> {
        tools
            .iter()
            .any(|t| !tool_in_path(t))
            .then(|| format!("Install {package} to preview {what}."))
    };
    match kind {
        DocKind::Pdf => need(&["pdftoppm", "pdfinfo"], "poppler-utils", "PDFs"),
        DocKind::Office => need(&["soffice", "pdftoppm", "pdfinfo"], "libreoffice and poppler-utils", "documents"),
        DocKind::Html => need(&["soffice", "pdftoppm", "pdfinfo"], "libreoffice and poppler-utils", "web pages"),
        DocKind::PostScript => need(&["gs", "pdftoppm", "pdfinfo"], "ghostscript and poppler-utils", "PostScript files"),
    }
}

pub fn cache_dir(sub: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".cache").join("wayfinder").join(sub);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Path + size + modification time (+ extra), so an edited file is rendered
/// again and an unchanged one only once.
pub fn cache_key(path: &Path, extra: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    stamp.hash(&mut h);
    extra.hash(&mut h);
    Some(h.finish())
}

/// Runs `cmd` to the end, killing its whole process group when `cancel` is
/// set or `limit` passes. LibreOffice starts helper processes, so killing only
/// the one we spawned would leave them running.
pub fn run_cancellable(mut cmd: Command, cancel: &AtomicBool, limit: Duration) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    let pid = child.id() as i32;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() { Ok(()) } else { Err(format!("exited with {status}")) };
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        let stale = cancel.load(Ordering::Relaxed);
        if stale || started.elapsed() > limit {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(if stale { "cancelled".into() } else { "took too long".into() });
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// The PDF to draw for `source`: the file itself, or a cached conversion.
pub fn pdf_for(source: &Path, kind: DocKind, cancel: &AtomicBool) -> Result<PathBuf, String> {
    if kind == DocKind::Pdf {
        return Ok(source.to_path_buf());
    }
    let dir = cache_dir("doc-pdf").ok_or("no cache folder")?;
    let key = cache_key(source, "pdf1").ok_or("cannot read the file")?;
    let cached = dir.join(format!("{key:016x}.pdf"));
    if cached.is_file() {
        return Ok(cached);
    }
    let work = dir.join(format!("{key:016x}.work"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let produced = if kind == DocKind::PostScript {
            let out = work.join("out.pdf");
            let mut cmd = Command::new("gs");
            cmd.args(["-q", "-dBATCH", "-dNOPAUSE", "-dSAFER", "-sDEVICE=pdfwrite", "-dEPSCrop"])
                .arg(format!("-sOutputFile={}", out.display()))
                .arg(source);
            run_cancellable(cmd, cancel, Duration::from_secs(90))?;
            out
        } else {
            let profile = dir.join("lo-profile");
            let mut cmd = Command::new("soffice");
            cmd.arg("--headless")
                .arg(format!("-env:UserInstallation=file://{}", profile.display()))
                .arg("--convert-to")
                .arg(if kind == DocKind::Html { "pdf:writer_web_pdf_Export" } else { "pdf" })
                .arg("--outdir")
                .arg(&work)
                .arg(source);
            run_cancellable(cmd, cancel, Duration::from_secs(180))?;
            let stem = source.file_stem().ok_or("no file name")?;
            work.join(stem).with_extension("pdf")
        };
        if !produced.is_file() {
            return Err("the converter made no PDF".to_string());
        }
        std::fs::rename(&produced, &cached).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result.map(|_| cached)
}

pub fn pdf_page_count(pdf: &Path) -> Result<u32, String> {
    let out = Command::new("pdfinfo")
        .arg(pdf)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("pdfinfo: {e}"))?;
    if !out.status.success() {
        return Err("pdfinfo could not read this PDF".into());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("Pages:").and_then(|n| n.trim().parse::<u32>().ok()))
        .filter(|n| *n > 0)
        .ok_or_else(|| "the PDF has no pages".into())
}

/// One page as a cached PNG `width` pixels wide.
pub fn render_pdf_page(pdf: &Path, page: u32, width: i32, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let dir = cache_dir("doc-pages").ok_or("no cache folder")?;
    let width = width.clamp(200, 2400);
    let key = cache_key(pdf, &format!("p{page}w{width}")).ok_or("cannot read the PDF")?;
    let cached = dir.join(format!("{key:016x}.png"));
    if cached.is_file() {
        return Ok(cached);
    }
    let prefix = dir.join(format!("{key:016x}.part"));
    let mut cmd = Command::new("pdftoppm");
    cmd.args(["-png", "-singlefile", "-f"])
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg("-scale-to-x")
        .arg(width.to_string())
        .arg("-scale-to-y")
        .arg("-1")
        .arg(pdf)
        .arg(&prefix);
    run_cancellable(cmd, cancel, Duration::from_secs(90))?;
    let made = prefix.with_extension("part.png");
    let made = if made.is_file() { made } else { PathBuf::from(format!("{}.png", prefix.display())) };
    if !made.is_file() {
        return Err("pdftoppm drew no page".into());
    }
    std::fs::rename(&made, &cached).map_err(|e| e.to_string())?;
    Ok(cached)
}

/// ImageMagick's turn at a picture: first frame only, upright, scaled down to
/// `width` and never up, written as a PNG.
pub fn magick_to_png(source: &str, out: &Path, width: i32) -> bool {
    Command::new("magick")
        .arg(format!("{source}[0]"))
        .args(["-auto-orient", "-resize"])
        .arg(format!("{}x>", width.clamp(16, 1024)))
        .arg(format!("png:{}", out.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && out.is_file()
}

/// A BMP the in-process loader cannot be trusted with: compressed (RLE) or a
/// negative height. gdk-pixbuf's BMP reader stalled on these (a BMP the old
/// Windows tools wrote, RLE8 with a top-down height), leaving the pane on
/// "Opening preview..." for good, so they go to ffmpeg/ImageMagick directly.
pub fn bmp_needs_converter(path: &str) -> bool {
    if ext_of(path) != "bmp" {
        return false;
    }
    let mut head = [0u8; 34];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut head).unwrap_or(0);
    bmp_header_is_unsafe(&head[..n])
}

pub fn bmp_header_is_unsafe(head: &[u8]) -> bool {
    if head.len() < 30 || &head[0..2] != b"BM" {
        return false;
    }
    let dib = u32::from_le_bytes([head[14], head[15], head[16], head[17]]);
    if dib == 12 {
        return false; // OS/2 core header: no compression, no negative height
    }
    let height = i32::from_le_bytes([head[22], head[23], head[24], head[25]]);
    let compression = if head.len() >= 34 {
        u32::from_le_bytes([head[30], head[31], head[32], head[33]])
    } else {
        0
    };
    height < 0 || compression == 1 || compression == 2
}

/// Seconds of video, from ffprobe.
pub fn video_duration_secs(path: &str) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok()
}

pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// What a Flash file's header says.
#[derive(Clone, Debug, PartialEq)]
pub struct SwfInfo {
    pub compression: &'static str,
    pub version: u8,
    pub width: f64,
    pub height: f64,
    pub frame_rate: f64,
    pub frames: u16,
}

/// Parses the SWF header. FWS is plain, CWS is zlib, ZWS is LZMA (decoded
/// with the `xz` tool when it is present).
pub fn parse_swf_header(data: &[u8]) -> Result<SwfInfo, String> {
    if data.len() < 8 {
        return Err("too short to be a Flash file".into());
    }
    let version = data[3];
    let (compression, body): (&'static str, Vec<u8>) = match &data[0..3] {
        b"FWS" => ("none", data[8..].to_vec()),
        b"CWS" => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(&data[8..])
                .take(64)
                .read_to_end(&mut out)
                .map_err(|e| format!("zlib part unreadable: {e}"))?;
            ("zlib", out)
        }
        b"ZWS" => ("LZMA", lzma_head(data)?),
        _ => return Err("not a Flash file (no FWS, CWS or ZWS signature)".into()),
    };
    if body.is_empty() {
        return Err("the Flash header is empty".into());
    }
    let nbits = (body[0] >> 3) as usize;
    let rect_bytes = (5 + nbits * 4 + 7) / 8;
    if body.len() < rect_bytes + 4 {
        return Err("the Flash header is cut short".into());
    }
    let mut pos = 5usize;
    let mut take = |bits: usize| -> i64 {
        let mut v: i64 = 0;
        for _ in 0..bits {
            let bit = (body[pos / 8] >> (7 - pos % 8)) & 1;
            v = (v << 1) | bit as i64;
            pos += 1;
        }
        if bits > 0 && v & (1 << (bits - 1)) != 0 {
            v -= 1 << bits;
        }
        v
    };
    let (xmin, xmax, ymin, ymax) = (take(nbits), take(nbits), take(nbits), take(nbits));
    let rest = &body[rect_bytes..];
    Ok(SwfInfo {
        compression,
        version,
        width: (xmax - xmin) as f64 / 20.0,
        height: (ymax - ymin) as f64 / 20.0,
        frame_rate: rest[1] as f64 + rest[0] as f64 / 256.0,
        frames: u16::from_le_bytes([rest[2], rest[3]]),
    })
}

fn lzma_head(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 17 || !tool_in_path("xz") {
        return Err("LZMA Flash header needs the xz tool".into());
    }
    // SWF: 8 bytes, 4 bytes packed length, 5 bytes properties, raw stream.
    // The .lzma file format wants the properties, then an 8-byte length.
    let mut lzma = data[12..17].to_vec();
    lzma.extend_from_slice(&[0xFF; 8]);
    lzma.extend_from_slice(&data[17..]);
    let mut child = Command::new("xz")
        .args(["--format=lzma", "-dc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no pipe")?;
    let feeder = std::thread::spawn(move || {
        use std::io::Write;
        let _ = stdin.write_all(&lzma);
    });
    let mut out = Vec::new();
    let _ = child.stdout.take().ok_or("no pipe")?.take(64).read_to_end(&mut out);
    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
    Ok(out)
}

pub fn read_swf_info(path: &str) -> Result<SwfInfo, String> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(1 << 20)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    parse_swf_header(&data)
}

/// What the pane tells him when no decoder could read a picture.
pub fn image_failure_hint() -> String {
    if !tool_in_path("ffmpeg") && !tool_in_path("magick") {
        "Install ffmpeg or imagemagick to open more picture formats.".into()
    } else {
        "Tried gdk-pixbuf, ffmpeg and ImageMagick.".into()
    }
}

/// Whether Ruffle is installed as a Flatpak (user or system).
pub fn ruffle_installed() -> bool {
    Command::new("flatpak")
        .args(["info", "rs.ruffle.Ruffle"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn swf_description(info: &Result<SwfInfo, String>) -> String {
    match info {
        Ok(i) => format!(
            "Flash file (SWF)\nVersion {}\nStage {:.0} x {:.0} pixels\n{:.1} frames per second\n{} frames\nCompression: {}",
            i.version, i.width, i.height, i.frame_rate, i.frames, i.compression
        ),
        Err(e) => format!("Flash file (SWF)\nThe header could not be read: {e}"),
    }
}

const ARCHIVE_EXT: [&str; 20] = [
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "lz", "lzma", "cab", "iso",
    "jar", "war", "apk", "rpm", "deb",
];

/// D-362: a file whose pane is a listing of what is inside it.
pub fn is_archive_path(path: &str) -> bool {
    ARCHIVE_EXT.contains(&ext_of(path).as_str())
}

/// Runs `cmd` under `timeout` so it cannot hang the pane, and returns its
/// standard output with at most `max_lines` lines.
fn capture_limited(program: &str, args: &[&str], secs: u32, max_lines: usize) -> Result<String, String> {
    let out = Command::new("timeout")
        .args(["-k", "2", &secs.to_string(), program])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    if out.status.code() == Some(124) {
        return Err(format!("{program} took longer than {secs} seconds"));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && text.trim().is_empty() {
        return Err(format!("{program} could not read this file"));
    }
    let mut lines: Vec<&str> = text.lines().collect();
    let more = lines.len() > max_lines;
    lines.truncate(max_lines);
    let mut shown = lines.join("\n");
    if more {
        shown.push_str("\n... (listing truncated)");
    }
    Ok(shown)
}

/// What `file -b` says the content is, or why it cannot say.
pub fn file_description(path: &str) -> String {
    match capture_limited("file", &["-b", "--", path], 5, 3) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => "unknown content".to_string(),
        Err(_) if !tool_in_path("file") => "Install file to see what this is.".to_string(),
        Err(why) => why,
    }
}

/// D-362: the words under the summary for a file with no picture, no text and
/// no player: the list of files inside an archive, or what the content is.
/// Slow (starts 7z or file); call it off the main thread.
pub fn describe_file(path: &str) -> String {
    if is_archive_path(path) {
        if !tool_in_path("7z") {
            return "Install 7zip to see what is inside this archive.".to_string();
        }
        return match capture_limited("7z", &["l", "-bd", "-y", "--", path], 20, 300) {
            Ok(list) => format!("Contents\n{}", list.trim_end()),
            Err(why) => format!("{}\nThe archive list is not available: {why}.", file_description(path)),
        };
    }
    format!("Content: {}", file_description(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A 550x400 pixel stage (11000 x 8000 twips), 24 fps, 12 frames.
    fn swf_body() -> Vec<u8> {
        let nbits = 15u32;
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |value: u32, count: u32| {
            for i in (0..count).rev() {
                bits.push((value >> i) & 1 == 1);
            }
        };
        put(nbits, 5);
        for v in [0u32, 11000, 0, 8000] {
            put(v, nbits);
        }
        let mut body: Vec<u8> = bits
            .chunks(8)
            .map(|c| c.iter().enumerate().fold(0u8, |b, (i, on)| b | ((*on as u8) << (7 - i))))
            .collect();
        body.extend_from_slice(&[0, 24]); // 24.0 frames per second, 8.8 fixed point
        body.extend_from_slice(&12u16.to_le_bytes());
        body
    }

    fn swf(signature: &[u8; 3], compress: bool) -> Vec<u8> {
        let body = swf_body();
        let mut out = signature.to_vec();
        out.push(9);
        out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        if compress {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&body).unwrap();
            out.extend_from_slice(&z.finish().unwrap());
        } else {
            out.extend_from_slice(&body);
        }
        out
    }

    #[test]
    fn swf_header_parses_plain_and_zlib_files_d362() {
        for (sig, compress, name) in [(b"FWS", false, "none"), (b"CWS", true, "zlib")] {
            let info = parse_swf_header(&swf(sig, compress)).unwrap();
            assert_eq!(
                info,
                SwfInfo { compression: name, version: 9, width: 550.0, height: 400.0, frame_rate: 24.0, frames: 12 }
            );
        }
        assert!(parse_swf_header(b"GIF89a..").is_err());
        assert!(parse_swf_header(b"FWS").is_err());
        assert!(swf_description(&parse_swf_header(&swf(b"FWS", false))).contains("Stage 550 x 400 pixels"));
    }

    #[test]
    fn lzma_swf_header_parses_when_xz_is_present_d362() {
        if !tool_in_path("xz") {
            return;
        }
        // Compress the body with xz in .lzma format, then rewrap it as SWF's
        // ZWS layout: 8 bytes, 4 bytes packed length, 5 bytes props, raw stream.
        let body = swf_body();
        let mut child = Command::new("xz")
            .args(["--format=lzma", "-zc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&body).unwrap();
        let lzma = child.wait_with_output().unwrap().stdout;
        let mut out = b"ZWS".to_vec();
        out.push(13);
        out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        out.extend_from_slice(&((lzma.len() - 13) as u32).to_le_bytes());
        out.extend_from_slice(&lzma[0..5]);
        out.extend_from_slice(&lzma[13..]);
        let info = parse_swf_header(&out).unwrap();
        assert_eq!((info.width, info.height, info.frames, info.compression), (550.0, 400.0, 12, "LZMA"));
    }

    fn bmp_head(height: i32, compression: u32) -> Vec<u8> {
        let mut h = b"BM".to_vec();
        h.extend_from_slice(&[0u8; 12]);
        h.extend_from_slice(&40u32.to_le_bytes());
        h.extend_from_slice(&32i32.to_le_bytes());
        h.extend_from_slice(&height.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&8u16.to_le_bytes());
        h.extend_from_slice(&compression.to_le_bytes());
        h
    }

    #[test]
    fn only_risky_bmp_headers_go_to_the_outside_decoders_d362() {
        assert!(bmp_header_is_unsafe(&bmp_head(8, 1)), "RLE8");
        assert!(bmp_header_is_unsafe(&bmp_head(8, 2)), "RLE4");
        assert!(bmp_header_is_unsafe(&bmp_head(-8192, 0)), "top-down");
        assert!(!bmp_header_is_unsafe(&bmp_head(8, 0)), "plain 24-bit stays in process");
        assert!(!bmp_header_is_unsafe(b"not a bitmap at all, just text....."));
    }

    #[test]
    fn documents_are_told_apart_by_extension_d362() {
        assert_eq!(doc_kind("/a/b.PDF"), Some(DocKind::Pdf));
        assert_eq!(doc_kind("/a/b.docx"), Some(DocKind::Office));
        assert_eq!(doc_kind("/a/b.eps"), Some(DocKind::PostScript));
        assert_eq!(doc_kind("/a/b.html"), Some(DocKind::Html));
        assert_eq!(doc_kind("/a/b.csv"), None, "csv stays a text preview");
        assert!(is_swf_path("/a/game.SWF"));
    }

    fn d362_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wf-d362-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn utf16_bytes(text: &str, big: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend_from_slice(if big { &[0xfe, 0xff] } else { &[0xff, 0xfe] });
        }
        for u in text.encode_utf16() {
            out.extend_from_slice(&if big { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        out
    }

    #[test]
    fn utf16_text_with_a_bom_is_shown_d362() {
        let reg = "Windows Registry Editor Version 5.00\r\n[HKEY_CURRENT_USER\\Test]\r\n\"A\"=\"\u{e9}t\u{e9}\"\r\n";
        for big in [false, true] {
            let got = crate::native_core::decode_text_bytes(&utf16_bytes(reg, big, true), false);
            assert_eq!(got.as_deref(), Some(reg));
        }
        // A real file goes through read_text_preview, the function the pane calls.
        let dir = d362_scratch("reg");
        let file = dir.join("settings.REG");
        std::fs::write(&file, utf16_bytes(reg, false, true)).unwrap();
        let shown = crate::native_core::read_text_preview(file.to_str().unwrap()).unwrap();
        assert!(shown.contains("Registry Editor") && shown.contains("\u{e9}t\u{e9}"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn utf16_text_without_a_bom_is_shown_d362() {
        let ini = "[Section]\r\nName=Value\r\nOther=Thing\r\n";
        for big in [false, true] {
            let got = crate::native_core::decode_text_bytes(&utf16_bytes(ini, big, false), false);
            assert_eq!(got.as_deref(), Some(ini));
        }
    }

    #[test]
    fn eight_bit_text_keeps_its_letters_and_data_stays_binary_d362() {
        // Windows-1252: e acute, euro sign, curly quote.
        let got = crate::native_core::decode_text_bytes(b"caf\xe9 \x80 \x93x\x94\n", false).unwrap();
        assert_eq!(got, "caf\u{e9} \u{20ac} \u{201c}x\u{201d}\n");
        // Valid UTF-8 and a UTF-8 BOM are unchanged.
        assert_eq!(crate::native_core::decode_text_bytes("\u{feff}h\u{e9}".as_bytes(), false).as_deref(), Some("h\u{e9}"));
        // A character split by the 256 KB cut is not an error.
        let mut cut = "ab".as_bytes().to_vec();
        cut.push(0xc3);
        assert_eq!(crate::native_core::decode_text_bytes(&cut, true).as_deref(), Some("ab"));
        // Zeros that are not UTF-16, and control-byte soup, stay binary.
        let mut data = vec![0u8; 64];
        data[10] = 7;
        assert!(crate::native_core::decode_text_bytes(&data, false).is_none());
        let soup: Vec<u8> = (1u8..32).cycle().take(300).collect();
        assert!(crate::native_core::decode_text_bytes(&soup, false).is_none());
    }

    #[test]
    fn upper_case_extensions_pick_the_same_viewer_d362() {
        assert_eq!(doc_kind("/x/SLIDES.PPT"), Some(DocKind::Office));
        assert_eq!(doc_kind("/x/Page.HTML"), Some(DocKind::Html));
        assert_eq!(doc_kind("/x/Report.PDF"), Some(DocKind::Pdf));
        assert!(is_swf_path("/x/GAME.SWF"));
        assert!(is_archive_path("/x/BACKUP.ZIP"));
        assert!(crate::native_core::is_image_path("/x/PHOTO.JPG"));
        assert!(crate::native_core::is_image_path("/x/Old.BMP"));
        assert!(crate::native_core::is_audio_path("/x/SONG.RMI"));
        assert!(crate::native_core::is_audio_path("/x/Tune.MID"));
    }

    #[test]
    fn midi_inside_a_riff_wrapper_has_a_length_d362() {
        let mut smf = b"MThd".to_vec();
        smf.extend_from_slice(&6u32.to_be_bytes());
        smf.extend_from_slice(&[0, 0, 0, 1, 0x01, 0xe0]); // format 0, one track, 480 ticks
        let track: Vec<u8> = vec![0, 0xff, 0x51, 3, 0x07, 0xa1, 0x20, 0x8f, 0x00, 0xff, 0x2f, 0];
        smf.extend_from_slice(b"MTrk");
        smf.extend_from_slice(&(track.len() as u32).to_be_bytes());
        smf.extend_from_slice(&track);
        assert_eq!(crate::native_core::midi_duration_from_bytes(&smf), Some(2.0));
        let mut rmi = b"RIFF".to_vec();
        rmi.extend_from_slice(&((smf.len() + 12) as u32).to_le_bytes());
        rmi.extend_from_slice(b"RMID");
        rmi.extend_from_slice(b"data");
        rmi.extend_from_slice(&(smf.len() as u32).to_le_bytes());
        rmi.extend_from_slice(&smf);
        assert_eq!(crate::native_core::midi_duration_from_bytes(&rmi), Some(2.0));
    }

    #[test]
    fn archive_listing_names_the_files_inside_d362() {
        if !tool_in_path("7z") {
            return;
        }
        let dir = d362_scratch("arc");
        std::fs::write(dir.join("inside-one.txt"), "hello").unwrap();
        std::fs::write(dir.join("inside-two.txt"), "world").unwrap();
        let archive = dir.join("pack.7z");
        let made = Command::new("7z")
            .args(["a", "-bd", "-y"])
            .arg(&archive)
            .arg(dir.join("inside-one.txt"))
            .arg(dir.join("inside-two.txt"))
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(made.success());
        let said = describe_file(archive.to_str().unwrap());
        assert!(said.starts_with("Contents") && said.contains("inside-one.txt") && said.contains("inside-two.txt"), "{said}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreadable_binary_says_what_it_is_d362() {
        let dir = d362_scratch("bin");
        let file = dir.join("carved.bin");
        let mut bytes = vec![0u8; 2048];
        bytes[..4].copy_from_slice(b"\x7fELF");
        std::fs::write(&file, &bytes).unwrap();
        let said = describe_file(file.to_str().unwrap());
        assert!(said.starts_with("Content: "), "{said}");
        assert!(said.len() > "Content: ".len());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Coverage harness (not run by default): prints, for each path in the
    /// file named by WAYFINDER_COVERAGE_LIST, which viewer the pane picks and
    /// whether it produced content. Run with `--ignored`.
    #[test]
    #[ignore]
    fn coverage_paths_d362() {
        let Ok(list) = std::env::var("WAYFINDER_COVERAGE_LIST") else { return };
        for path in std::fs::read_to_string(list).unwrap().lines() {
            let kind = if crate::native_core::is_image_path(path) {
                "image"
            } else if crate::native_core::is_audio_path(path) {
                "audio"
            } else if crate::native_core::is_video_path(path) {
                "video"
            } else if doc_kind(path).is_some() {
                "doc"
            } else if is_swf_path(path) {
                "swf"
            } else if is_archive_path(path) {
                "archive"
            } else {
                match crate::native_core::read_text_preview(path) {
                    Ok(_) => "text",
                    Err(e) => e.leak(),
                }
            };
            println!("{kind}\t{path}");
        }
    }
}
