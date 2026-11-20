// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Kadaken

use adw::gtk::gio;
use adw::gtk::gio::prelude::FileExt;
use adw::gtk::glib;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const IMG_EXT: [&str; 16] = [
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "svg", "ico", "avif", "heic",
    // D-251: these three were missing, so they were not even treated as images.
    "heif", "avifs", "jxl",
    // D-322: a Canon camera's video thumbnail (MVI_0001.THM) is a plain JPEG.
    "thm",
];
/// Every sound-file extension the preview offers to play.
///
/// It is long on purpose. The old list had 13 entries, and a file it did not
/// name got the plain-text summary with no way to hear it -- which is the same
/// dead end as having no audio preview at all, just for fewer files. These are
/// the extensions GStreamer's own typefinder and demuxers cover, so a file
/// named here either plays or says in the pane why it cannot; neither outcome
/// can crash, because `audio_preview` never hands a file to GtkMediaFile.
///
/// Playlists (.m3u, .pls, .cue) are deliberately absent: they are text files
/// listing other files, so the text preview is the right one for them.
const AUDIO_EXT: [&str; 50] = [
    // MPEG audio
    "mp3", "mp2", "mp1", "mpga", "m4a", "m4b", "m4r", "aac", "adts", "aa3", "oma",
    // Free and lossless
    "flac", "alac", "ape", "wv", "tta", "shn", "tak",
    // Uncompressed and Apple
    "wav", "wave", "w64", "aiff", "aif", "aifc", "caf", "au", "snd", "voc",
    // Ogg family
    "ogg", "oga", "opus", "spx",
    // Containers that normally hold only sound
    "mka", "weba",
    // Windows and broadcast
    "wma", "ac3", "eac3", "dts",
    // Telephony and handheld
    "amr", "awb", "3ga", "gsm", "ra",
    // DSD and Musepack
    "dsf", "dff", "mpc",
    // MIDI (played through the fluidsynth GStreamer plugin, D-327)
    "mid", "midi", "kar", "rmi",
];
const VIDEO_EXT: [&str; 12] = [
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "wmv", "flv", "ts", "3gp", "ogv", "mpeg",
];

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub source: EntrySource,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub total: u64,
    pub available: u64,
}

/// Where a browse location lives: a real directory, or a folder inside a ZIP
/// being browsed read-only. `internal` is empty at the ZIP's virtual root.
/// Carried by tabs/history/breadcrumbs instead of an ambiguous path string,
/// so a ZIP member can never be mistaken for a mutable disk path (D-188).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    RealDir(PathBuf),
    ZipDir { archive: PathBuf, internal: String },
}

impl Location {
    pub fn real(path: impl Into<PathBuf>) -> Self {
        Self::RealDir(path.into())
    }

    pub fn zip_root(archive: impl Into<PathBuf>) -> Self {
        Self::ZipDir { archive: archive.into(), internal: String::new() }
    }

    pub fn is_zip(&self) -> bool {
        matches!(self, Self::ZipDir { .. })
    }

    /// The real filesystem path, when this location is a real directory.
    /// `None` for a `ZipDir`, so a caller that needs an actual on-disk path
    /// can never get a display string instead (D-188, REJECT 1).
    pub fn real_path(&self) -> Option<&Path> {
        match self {
            Self::RealDir(path) => Some(path),
            Self::ZipDir { .. } => None,
        }
    }

    /// Owned `String` form of `real_path`, for the many call sites that need
    /// an owned real path rather than a borrow. Still `None` for a `ZipDir`.
    pub fn real_path_string(&self) -> Option<String> {
        self.real_path().map(|p| p.to_string_lossy().into_owned())
    }

    /// A human-readable form safe for any GTK widget text (status line,
    /// error message, window title).
    pub fn display_string(&self) -> String {
        match self {
            Self::RealDir(path) => path.to_string_lossy().into_owned(),
            Self::ZipDir { archive, internal } => {
                let archive = archive.to_string_lossy();
                if internal.is_empty() {
                    archive.into_owned()
                } else {
                    format!("{archive} » {internal}")
                }
            }
        }
    }

    /// One level up: deeper into the archive pops the last internal segment;
    /// at the archive's virtual root, Up returns to the real containing
    /// folder (D-188's "Up at the ZIP root returns to the real containing
    /// folder").
    pub fn parent(&self) -> Location {
        match self {
            Self::RealDir(path) => {
                Self::RealDir(path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")))
            }
            Self::ZipDir { archive, internal } => {
                if internal.is_empty() {
                    Self::RealDir(archive.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")))
                } else {
                    match internal.rsplit_once('/') {
                        Some((parent, _)) => Self::ZipDir { archive: archive.clone(), internal: parent.to_string() },
                        None => Self::ZipDir { archive: archive.clone(), internal: String::new() },
                    }
                }
            }
        }
    }

    pub fn child_dir(&self, name: &str) -> Location {
        match self {
            Self::RealDir(path) => Self::RealDir(path.join(name)),
            Self::ZipDir { archive, internal } => {
                let internal = if internal.is_empty() { name.to_string() } else { format!("{internal}/{name}") };
                Self::ZipDir { archive: archive.clone(), internal }
            }
        }
    }

    /// Breadcrumb segments from the ZIP's containing real folder onward: the
    /// archive's own file name, then each internal folder, each paired with
    /// the `Location` that activating that crumb navigates to. `None` when
    /// this is a real location (the existing real-path breadcrumb builder
    /// still applies).
    pub fn zip_breadcrumb_tail(&self) -> Option<Vec<(String, Location)>> {
        let Self::ZipDir { archive, internal } = self else { return None };
        let archive_name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut crumbs = vec![(archive_name, Location::zip_root(archive.clone()))];
        let mut accumulated = String::new();
        if !internal.is_empty() {
            for part in internal.split('/') {
                accumulated = if accumulated.is_empty() { part.to_string() } else { format!("{accumulated}/{part}") };
                crumbs.push((part.to_string(), Self::ZipDir { archive: archive.clone(), internal: accumulated.clone() }));
            }
        }
        Some(crumbs)
    }
}

/// Message shown for every refused ZIP mutation attempt (menu, keyboard,
/// programmatic, drag/drop) — one exact string, never paraphrased.
pub const ZIP_READ_ONLY_MESSAGE: &str = "ZIP folders are read-only";

/// A single indexed member of a specific archive, carrying enough identity
/// to revalidate the archive before Open/Copy act on it (D-188): a stale or
/// changed archive must be caught before a row from it reaches the
/// filesystem, not after.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ZipMemberRef {
    pub archive: PathBuf,
    pub internal: String,
    /// Size and CRC-32 captured from the archive at listing time (D-188
    /// REJECT 5), so Open/Copy can reject a changed or replaced archive
    /// instead of silently streaming whatever the current file now has at
    /// that name. 0/0 for a directory row, which never reaches extraction.
    pub size: u64,
    pub crc32: u32,
    /// The archive file's byte length at listing time (D-188 REJECT 5):
    /// archive identity, checked alongside the member's own size/CRC so a
    /// wholesale archive replacement is caught even on the rare chance it
    /// preserves one member's name, size and CRC.
    pub archive_len: u64,
    /// This member's position in the archive's own central directory at
    /// listing time (D-188 REJECT 5): exact entry identity, checked against a
    /// fresh full re-index of the live archive before streaming — never a
    /// scan that stops at the first matching name.
    pub member_index: usize,
}

/// Where a row's bytes actually come from, carried instead of a serialized
/// pseudo-path so a ZIP member can never reach a real-filesystem-only API
/// (picker, preview, launcher, drag, rename, trash) by accident: `Real`
/// converts explicitly to `PathBuf`, `ZipMember` never does.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntrySource {
    Real(PathBuf),
    ZipMember(ZipMemberRef),
}

impl EntrySource {
    pub fn real(path: impl Into<PathBuf>) -> Self {
        Self::Real(path.into())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn zip_member(
        archive: impl Into<PathBuf>,
        internal: impl Into<String>,
        size: u64,
        crc32: u32,
        archive_len: u64,
        member_index: usize,
    ) -> Self {
        Self::ZipMember(ZipMemberRef {
            archive: archive.into(),
            internal: internal.into(),
            size,
            crc32,
            archive_len,
            member_index,
        })
    }

    pub fn is_zip(&self) -> bool {
        matches!(self, Self::ZipMember(_))
    }

    /// The real filesystem path, when this row is a real entry. `None` for a
    /// `ZipMember`, so a caller that needs an actual on-disk path can never
    /// get a marker-encoded string instead (D-188).
    pub fn real_path(&self) -> Option<&Path> {
        match self {
            Self::Real(path) => Some(path),
            Self::ZipMember(_) => None,
        }
    }

    /// Owned `String` form of `real_path`, for the many call sites that need
    /// an owned real path rather than a borrow. Still `None` for a `ZipMember`.
    pub fn real_path_string(&self) -> Option<String> {
        self.real_path().map(|p| p.to_string_lossy().into_owned())
    }

    pub fn display_string(&self) -> String {
        self.to_location().display_string()
    }

    /// The browse-state `Location` this row would navigate to if it is a
    /// directory. Only `display_string`/navigation use this; a
    /// real-filesystem-only API must match on `EntrySource` itself instead,
    /// so a ZIP member can't reach it through this conversion.
    pub fn to_location(&self) -> Location {
        match self {
            Self::Real(path) => Location::RealDir(path.clone()),
            Self::ZipMember(ZipMemberRef { archive, internal, .. }) => {
                Location::ZipDir { archive: archive.clone(), internal: internal.clone() }
            }
        }
    }
}

/// Where a clipboard row's bytes come from. Only a `Real` source may ever
/// populate the clipboard (`cut_selection`/`copy_selection` refuse a ZIP row
/// before it is built), but the type itself is what makes a `ZipMember`
/// unrepresentable here rather than relying on that check alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardSource {
    Real(PathBuf),
}

impl ClipboardSource {
    pub fn real_path(&self) -> &Path {
        match self {
            Self::Real(path) => path,
        }
    }
}

/// D-232: the three clipboard formats file Copy, Cut and Paste share with other
/// windows and other programs.
pub const CLIP_URI_LIST: &str = "text/uri-list";
pub const CLIP_GNOME_FILES: &str = "x-special/gnome-copied-files";
pub const CLIP_KDE_CUT: &str = "application/x-kde-cutselection";
/// The most bytes one paste reads from the clipboard for one format.
pub const CLIP_READ_LIMIT: usize = 4 * 1024 * 1024;

/// What a paste does with the files on the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipIntent {
    Copy,
    Cut,
}

impl ClipIntent {
    /// The word the GNOME form and the status line use.
    pub fn word(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
        }
    }
}

/// One file Copy or Cut in the three forms, offered together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipPayload {
    pub gnome: Vec<u8>,
    pub uri_list: Vec<u8>,
    pub kde_cut: Vec<u8>,
}

/// Encode real paths for the clipboard. The URIs come from GIO, so a space, a
/// percent sign, `#`, a newline or a non-ASCII name survives the trip.
pub fn clip_encode(paths: &[PathBuf], intent: ClipIntent) -> Result<ClipPayload, String> {
    if paths.is_empty() {
        return Err("there is nothing to put on the clipboard".to_string());
    }
    let mut uris = Vec::with_capacity(paths.len());
    for path in paths {
        if !path.is_absolute() {
            return Err(format!("{} is not a full path", path.display()));
        }
        uris.push(gio::File::for_path(path).uri().to_string());
    }
    let uri_list: String = uris.iter().map(|uri| format!("{uri}\r\n")).collect();
    Ok(ClipPayload {
        gnome: format!("{}\n{}", intent.word(), uris.join("\n")).into_bytes(),
        uri_list: uri_list.into_bytes(),
        kde_cut: match intent {
            ClipIntent::Copy => b"0".to_vec(),
            ClipIntent::Cut => b"1".to_vec(),
        },
    })
}

/// The files a paste found on the clipboard, and what to do with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipFiles {
    pub intent: ClipIntent,
    pub paths: Vec<PathBuf>,
}

/// What one paste read from the clipboard, by the form it found. The GNOME form
/// is used whenever it is offered. Without it the URI list is used, with the
/// KDE marker when one is offered. A URI list alone is a Copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipRead {
    /// The GNOME form, and the KDE marker when the same offer carries one.
    Gnome { gnome: Vec<u8>, kde_cut: Option<Vec<u8>> },
    /// A URI list with the KDE marker, read once before the list and once after.
    Kde { cut_before: Vec<u8>, uris: Vec<u8>, cut_after: Vec<u8> },
    /// A URI list and nothing else.
    UriOnly { uris: Vec<u8> },
}

/// Turn what a paste read into local paths and an intent, or say why not. One
/// entry that is not a plain local file refuses the whole paste, and a marker
/// that is missing its value, malformed or in disagreement is never guessed.
pub fn clip_decode(read: &ClipRead) -> Result<ClipFiles, String> {
    match read {
        ClipRead::Gnome { gnome, kde_cut } => {
            let text = clip_text(gnome)?;
            let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
            let intent = match first.trim_end_matches('\r') {
                "copy" => ClipIntent::Copy,
                "cut" => ClipIntent::Cut,
                _ => return Err("the clipboard does not say copy or cut".to_string()),
            };
            if let Some(marker) = kde_cut {
                if clip_kde_intent(marker)? != intent {
                    return Err("the clipboard's copy and cut markers disagree".to_string());
                }
            }
            Ok(ClipFiles { intent, paths: clip_paths(rest, false)? })
        }
        ClipRead::Kde { cut_before, uris, cut_after } => {
            if cut_before != cut_after {
                return Err("the clipboard changed while it was read".to_string());
            }
            let intent = clip_kde_intent(cut_before)?;
            Ok(ClipFiles { intent, paths: clip_paths(clip_text(uris)?, true)? })
        }
        ClipRead::UriOnly { uris } => {
            Ok(ClipFiles { intent: ClipIntent::Copy, paths: clip_paths(clip_text(uris)?, true)? })
        }
    }
}

/// Clipboard bytes as text. Some programs end the data with a zero byte.
fn clip_text(data: &[u8]) -> Result<&str, String> {
    let end = data.iter().rposition(|byte| *byte != 0).map_or(0, |last| last + 1);
    std::str::from_utf8(&data[..end]).map_err(|_| "the clipboard data is not text".to_string())
}

/// KDE writes `1` for a cut and `0` for a copy. Anything else is refused.
fn clip_kde_intent(marker: &[u8]) -> Result<ClipIntent, String> {
    match clip_text(marker).map(str::trim) {
        Ok("0") => Ok(ClipIntent::Copy),
        Ok("1") => Ok(ClipIntent::Cut),
        _ => Err("the clipboard's cut marker is not 0 or 1".to_string()),
    }
}

/// One URI per line. `comments` is the `text/uri-list` rule that a line starting
/// with `#` is a note. The same file listed twice is kept once.
fn clip_paths(text: &str, comments: bool) -> Result<Vec<PathBuf>, String> {
    let own_host = glib::host_name();
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for line in text.split('\n') {
        let uri = line.trim();
        if uri.is_empty() || (comments && uri.starts_with('#')) {
            continue;
        }
        let path = clip_local_path(uri, own_host.as_str())?;
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err("the clipboard holds no files".to_string());
    }
    Ok(paths)
}

/// The local path one clipboard URI names. Only `file` addresses on this
/// computer are accepted; nothing is mounted, fetched or looked up.
fn clip_local_path(uri: &str, own_host: &str) -> Result<PathBuf, String> {
    // What the status line names: short, and never a control character.
    let shown: String = uri.chars().take(80).map(|c| if c.is_control() { '\u{FFFD}' } else { c }).collect();
    // GLib drops everything from a bare `#` or `?` on, which would name a
    // different file. A real `#` or `?` in a name arrives percent-encoded.
    if uri.contains(['\0', '#', '?']) {
        return Err(format!("{shown} is not a valid file address"));
    }
    let Some(scheme) = glib::uri_parse_scheme(uri) else {
        return Err(format!("{shown} is not a file address"));
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err(format!("{shown} is not a local file ({scheme} is not supported)"));
    }
    let Ok((path, host)) = glib::filename_from_uri(uri) else {
        return Err(format!("{shown} is not a valid file address"));
    };
    if let Some(host) = host.as_deref().filter(|host| !host.is_empty()) {
        if !host.eq_ignore_ascii_case("localhost") && !host.eq_ignore_ascii_case(own_host) {
            return Err(format!("{shown} is on another computer"));
        }
    }
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(format!("{shown} does not name a file or folder"));
    }
    Ok(path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZipNameError {
    Absolute,
    ParentTraversal,
    NulByte,
    BackslashSeparator,
    AmbiguousNormalization,
    DuplicatePath,
    FileDirectoryCollision,
    SpecialEntry,
}

impl std::fmt::Display for ZipNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute => write!(f, "names an absolute path"),
            Self::ParentTraversal => write!(f, "escapes the archive with \"..\""),
            Self::NulByte => write!(f, "contains a NUL byte"),
            Self::BackslashSeparator => write!(f, "uses an unsafe separator"),
            Self::AmbiguousNormalization => write!(f, "normalizes ambiguously"),
            Self::DuplicatePath => write!(f, "duplicates another entry's path"),
            Self::FileDirectoryCollision => write!(f, "collides as a file and a directory"),
            Self::SpecialEntry => write!(f, "is a symlink or special entry"),
        }
    }
}

/// True when a Unix mode's file-type bits (S_IFMT) name anything other than
/// a regular file or a directory — a symlink, device, FIFO or socket — which
/// D-188 must reject at index time, not just symlinks (REJECT 6).
pub fn is_disallowed_unix_mode(mode: u32) -> bool {
    const S_IFMT: u32 = 0o170000;
    const S_IFREG: u32 = 0o100000;
    const S_IFDIR: u32 = 0o040000;
    let file_type = mode & S_IFMT;
    file_type != 0 && file_type != S_IFREG && file_type != S_IFDIR
}

/// Normalizes and validates one ZIP member name against every unsafe form
/// this round must reject before indexing or streaming it (D-188). Returns
/// the normalized logical path (forward-slash, no leading/trailing slash)
/// on success. Caller tracks `seen` paths and `seen_as_dir` across the whole
/// archive to catch duplicates and file/directory collisions.
pub fn validate_zip_member_name(
    raw: &str,
    seen: &HashSet<String>,
    seen_as_dir: &HashSet<String>,
    is_dir: bool,
) -> Result<String, ZipNameError> {
    if raw.contains('\0') {
        return Err(ZipNameError::NulByte);
    }
    if raw.contains('\\') {
        return Err(ZipNameError::BackslashSeparator);
    }
    if raw.starts_with('/') || raw.starts_with("//") {
        return Err(ZipNameError::Absolute);
    }
    // A Windows drive-letter form ("C:/x") is also an absolute escape.
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return Err(ZipNameError::Absolute);
    }
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ZipNameError::ParentTraversal);
    }
    let mut normalized_parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" => return Err(ZipNameError::AmbiguousNormalization),
            "." => return Err(ZipNameError::AmbiguousNormalization),
            ".." => return Err(ZipNameError::ParentTraversal),
            p => normalized_parts.push(p),
        }
    }
    let normalized = normalized_parts.join("/");
    if seen.contains(&normalized) {
        return Err(ZipNameError::DuplicatePath);
    }
    if is_dir {
        if seen.contains(&normalized) {
            return Err(ZipNameError::FileDirectoryCollision);
        }
    } else if seen_as_dir.contains(&normalized) {
        return Err(ZipNameError::FileDirectoryCollision);
    }
    Ok(normalized)
}

/// Resource bounds enforced while indexing and streaming a ZIP (D-188). Kept
/// as one named place so a limit is never silently re-chosen at a call site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZipLimits {
    pub max_members: usize,
    pub max_name_len: usize,
    pub max_total_claimed_bytes: u64,
    pub max_entry_claimed_bytes: u64,
    /// Reject an entry whose claimed uncompressed size is more than this many
    /// times its compressed size — a cheap guard against a decompression bomb
    /// before a single byte of that member is ever read.
    pub max_compression_ratio: u64,
}

pub const DEFAULT_ZIP_LIMITS: ZipLimits = ZipLimits {
    max_members: 200_000,
    max_name_len: 1024,
    max_total_claimed_bytes: 20 * 1024 * 1024 * 1024,
    max_entry_claimed_bytes: 4 * 1024 * 1024 * 1024,
    max_compression_ratio: 1200,
};

/// The limits every production ZIP read actually enforces. Identical to
/// `DEFAULT_ZIP_LIMITS` outside tests; a test may inject a tighter set
/// through `TEST_ZIP_LIMITS` to exercise a boundary without needing a
/// real multi-gigabyte fixture on disk (D-188 #55-#60). A process-wide
/// `Mutex`, not a thread-local: the real listing path runs the indexing
/// call inside `gio::spawn_blocking`, a worker thread distinct from the
/// test thread that sets the override, so a thread-local override would
/// never be seen there.
#[cfg(not(test))]
fn effective_zip_limits() -> ZipLimits {
    DEFAULT_ZIP_LIMITS
}

#[cfg(test)]
static TEST_ZIP_LIMITS: std::sync::Mutex<Option<ZipLimits>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn effective_zip_limits() -> ZipLimits {
    TEST_ZIP_LIMITS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or(DEFAULT_ZIP_LIMITS)
}

#[cfg(test)]
fn set_test_zip_limits(limits: Option<ZipLimits>) {
    *TEST_ZIP_LIMITS.lock().unwrap_or_else(|e| e.into_inner()) = limits;
}

/// Serializes every test that can read `effective_zip_limits()` through
/// listing, revalidation, Open or Copy Out — with or without its own
/// override — so two such tests running concurrently (the default test
/// harness) cannot observe or restore each other's value. Held by
/// `TestZipLimitsGuard` for its whole lifetime, kept separate from
/// `TEST_ZIP_LIMITS` itself so a production worker thread's plain read of
/// the value can never deadlock behind a guard that also held the value
/// mutex.
#[cfg(test)]
pub static TEST_ZIP_LIMITS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// `TEST_ZIP_LIMITS_LOCK` is a plain, non-reentrant `std::sync::Mutex`: it
// exists to serialize *different test threads* against each other, not to
// forbid a test from nesting a second `TestZipLimitsGuard` inside its own
// `catch_unwind` on the same thread (the unwind-safety proofs below need
// exactly that). This counts outstanding guards on the current thread so
// only the outermost one actually locks/unlocks the mutex.
#[cfg(test)]
thread_local! {
    static ZIP_LIMITS_LOCK_DEPTH: std::cell::Cell<u32> = std::cell::Cell::new(0);
}

/// Test-only, unwind-safe override of the ZIP limits every production
/// listing/Open/Copy Out call reads through `effective_zip_limits()`.
/// Captures the exact prior `Option<ZipLimits>` on construction and
/// restores it in `Drop`, including on panic/unwind and every early-return
/// path, so one test's boundary fixture can never leak into another's —
/// there must be no happy-path-only tail restore of this process-wide
/// state (D-188 #55-#61). Holds `TEST_ZIP_LIMITS_LOCK` for its entire
/// lifetime; a test with nothing to override still constructs one with
/// `None` so it is serialized against every test that does.
#[cfg(test)]
pub struct TestZipLimitsGuard {
    _lock: Option<std::sync::MutexGuard<'static, ()>>,
    prior: Option<ZipLimits>,
}

#[cfg(test)]
impl TestZipLimitsGuard {
    pub fn new(limits: Option<ZipLimits>) -> Self {
        let depth = ZIP_LIMITS_LOCK_DEPTH.with(|d| d.get());
        let lock = if depth == 0 {
            Some(TEST_ZIP_LIMITS_LOCK.lock().unwrap_or_else(|e| e.into_inner()))
        } else {
            None
        };
        ZIP_LIMITS_LOCK_DEPTH.with(|d| d.set(depth + 1));
        let prior = *TEST_ZIP_LIMITS.lock().unwrap_or_else(|e| e.into_inner());
        set_test_zip_limits(limits);
        TestZipLimitsGuard { _lock: lock, prior }
    }

    pub fn set(&self, limits: Option<ZipLimits>) {
        set_test_zip_limits(limits);
    }
}

#[cfg(test)]
impl Drop for TestZipLimitsGuard {
    fn drop(&mut self) {
        set_test_zip_limits(self.prior);
        ZIP_LIMITS_LOCK_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

#[cfg(test)]
mod zip_limits_guard_tests_d188 {
    use super::*;

    #[test]
    fn restores_a_prior_value_through_a_panic_d188() {
        let outer = TestZipLimitsGuard::new(Some(ZipLimits { max_members: 111, ..DEFAULT_ZIP_LIMITS }));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let inner = TestZipLimitsGuard::new(Some(ZipLimits { max_members: 222, ..DEFAULT_ZIP_LIMITS }));
            assert_eq!(effective_zip_limits().max_members, 222);
            let _ = &inner;
            panic!("simulated mid-test failure");
        }));
        assert!(result.is_err());
        assert_eq!(effective_zip_limits().max_members, 111, "the outer override must survive the inner guard's panic unwind");
        drop(outer);
        assert_eq!(effective_zip_limits(), DEFAULT_ZIP_LIMITS, "once every guard is dropped, limits must fall back to the shipped defaults");
    }

    #[test]
    fn restores_absence_through_a_panic_d188() {
        // Serialized for its complete limits-reading lifetime against every
        // other test that can reach `effective_zip_limits()` (D-188 DEFECT
        // 2); installs no override of its own, so the prior it captures and
        // restores is the shipped absence.
        let outer = TestZipLimitsGuard::new(None);
        assert_eq!(effective_zip_limits(), DEFAULT_ZIP_LIMITS, "no override installed anywhere must mean the shipped defaults");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = TestZipLimitsGuard::new(Some(ZipLimits { max_members: 333, ..DEFAULT_ZIP_LIMITS }));
            assert_eq!(effective_zip_limits().max_members, 333);
            let _ = &guard;
            panic!("simulated mid-test failure");
        }));
        assert!(result.is_err());
        assert_eq!(effective_zip_limits(), DEFAULT_ZIP_LIMITS, "a panic while overridden must restore to the prior absence, not leak the override");
        drop(outer);
    }

    /// D-188 #55-#60 DEFECT 3: with no override installed anywhere, the
    /// production hook must read back exactly the shipped `DEFAULT_ZIP_LIMITS`
    /// field values — this is what makes a typo or divergence in the real
    /// shipped constants fail, not just the override mechanism itself.
    #[test]
    fn shipped_constants_are_the_effective_limits_with_no_override_d188() {
        let _guard = TestZipLimitsGuard::new(None);
        let effective = effective_zip_limits();
        assert_eq!(effective.max_members, 200_000);
        assert_eq!(effective.max_name_len, 1024);
        assert_eq!(effective.max_total_claimed_bytes, 20 * 1024 * 1024 * 1024);
        assert_eq!(effective.max_entry_claimed_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(effective.max_compression_ratio, 1200);
        assert_eq!(effective, DEFAULT_ZIP_LIMITS);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ZipIndexError {
    Cancelled,
    Open(String),
    Encrypted(String),
    Unsupported(String),
    BadName(String, ZipNameError),
    TooManyMembers,
    NameTooLong(String),
    EntryTooLarge(String),
    ArchiveTooLarge,
    SuspiciousCompressionRatio(String),
    Corrupt(String),
}

/// A raw ZIP member name may legally contain control bytes, including an
/// embedded NUL — but a status message built from one is later handed to a
/// GTK label, whose GLib FFI layer converts the whole string to a C string
/// and panics on an interior NUL. Every name interpolated into a
/// `ZipIndexError`/`ZipNameError` message must go through this escape first.
fn escape_zip_name_for_display(name: &str) -> String {
    name.escape_default().to_string()
}

impl std::fmt::Display for ZipIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "archive loading was cancelled"),
            Self::Open(e) => write!(f, "could not open archive: {e}"),
            Self::Encrypted(name) => {
                write!(f, "\"{}\" is encrypted and cannot be browsed", escape_zip_name_for_display(name))
            }
            Self::Unsupported(name) => {
                write!(f, "\"{}\" uses an unsupported format", escape_zip_name_for_display(name))
            }
            Self::BadName(name, why) => write!(f, "\"{}\" {why}", escape_zip_name_for_display(name)),
            Self::TooManyMembers => write!(f, "archive has too many entries"),
            Self::NameTooLong(name) => write!(f, "\"{}\" has too long a name", escape_zip_name_for_display(name)),
            Self::EntryTooLarge(name) => write!(f, "\"{}\" is too large", escape_zip_name_for_display(name)),
            Self::ArchiveTooLarge => write!(f, "archive's claimed total size is too large"),
            Self::SuspiciousCompressionRatio(name) => {
                write!(f, "\"{}\" has a suspicious compression ratio", escape_zip_name_for_display(name))
            }
            Self::Corrupt(e) => write!(f, "archive is corrupt or truncated: {e}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZipIndexEntry {
    /// Normalized logical path, forward-slash, no leading/trailing slash.
    pub logical_path: String,
    pub is_dir: bool,
    pub size: u64,
    /// CRC-32 of the uncompressed bytes, from the central directory at list
    /// time (D-188 REJECT 5). 0 for a directory (real or synthesized) — never
    /// read back for one.
    pub crc32: u32,
    /// This entry's position in the archive's own central directory (D-188
    /// REJECT 5), so a member can be re-located by physical identity, not
    /// just by a name that a replacement archive could reuse at a different
    /// position. `usize::MAX` for a synthesized implicit directory, which has
    /// no physical entry of its own and never streams.
    pub physical_index: usize,
}

/// Reads only the ZIP central directory (no member bytes) and returns every
/// real entry plus implicit parent directories synthesized from the paths,
/// validated and limit-checked (D-188). Never extracts file content.
pub fn index_zip<R: Read + std::io::Seek>(
    reader: R,
    limits: &ZipLimits,
    cancelled: &AtomicBool,
) -> Result<Vec<ZipIndexEntry>, ZipIndexError> {
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| ZipIndexError::Open(e.to_string()))?;
    if archive.len() > limits.max_members {
        return Err(ZipIndexError::TooManyMembers);
    }
    // One logical namespace map, built incrementally as entries are read, so
    // a collision is rejected regardless of which order the two colliding
    // entries appear in the archive (D-188 REJECT 6). `true` = reserved as a
    // directory (explicit or an ancestor of something else), `false` =
    // reserved as a file.
    let mut namespace: HashMap<String, bool> = HashMap::new();
    let mut explicit_entries: Vec<(String, bool, u64, u32, usize)> = Vec::with_capacity(archive.len());
    let mut seen_explicit_dirs: HashSet<String> = HashSet::new();
    let mut total_claimed: u64 = 0;

    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ZipIndexError::Cancelled);
        }
        #[cfg(test)]
        {
            let micros = TEST_INDEX_ENTRY_DELAY_MICROS.load(Ordering::Relaxed);
            if micros > 0 {
                std::thread::sleep(std::time::Duration::from_micros(micros));
            }
        }
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| ZipIndexError::Corrupt(e.to_string()))?;
        // zip 9 can fail to decode a member name (bad UTF-8 flags, a broken
        // extra field). A name we cannot read is a name we cannot validate, so
        // the archive is refused rather than guessed at.
        let raw_name = entry
            .name()
            .map_err(|e| ZipIndexError::Corrupt(e.to_string()))?
            .to_string();
        if raw_name.len() > limits.max_name_len {
            return Err(ZipIndexError::NameTooLong(raw_name));
        }
        if entry.encrypted() {
            return Err(ZipIndexError::Encrypted(raw_name));
        }
        let is_dir = entry.is_dir();
        // Reject every non-regular/non-directory Unix mode (symlinks,
        // devices, FIFOs, sockets), not symlinks alone.
        if entry.unix_mode().is_some_and(is_disallowed_unix_mode) {
            return Err(ZipIndexError::BadName(raw_name, ZipNameError::SpecialEntry));
        }
        #[allow(deprecated)]
        let is_unsupported_method = matches!(entry.compression(), zip::CompressionMethod::Unsupported(_));
        if !is_dir && is_unsupported_method {
            return Err(ZipIndexError::Unsupported(raw_name));
        }

        let normalized = validate_zip_member_name(&raw_name, &HashSet::new(), &HashSet::new(), is_dir)
            .map_err(|why| ZipIndexError::BadName(raw_name.clone(), why))?;

        let size = entry.size();
        let crc32 = entry.crc32();
        let compressed = entry.compressed_size().max(1);
        if !is_dir {
            if size > limits.max_entry_claimed_bytes {
                return Err(ZipIndexError::EntryTooLarge(raw_name));
            }
            if size / compressed > limits.max_compression_ratio {
                return Err(ZipIndexError::SuspiciousCompressionRatio(raw_name));
            }
            total_claimed = total_claimed.saturating_add(size);
            if total_claimed > limits.max_total_claimed_bytes {
                return Err(ZipIndexError::ArchiveTooLarge);
            }
        }

        // Reserve every ancestor as a directory immediately, rejecting an
        // ancestor already reserved as a file, independent of archive order.
        let mut parent = normalized.rsplit_once('/').map(|(p, _)| p);
        while let Some(p) = parent {
            if !p.is_empty() {
                match namespace.get(p) {
                    Some(false) => return Err(ZipIndexError::BadName(raw_name, ZipNameError::FileDirectoryCollision)),
                    Some(true) => break,
                    None => {
                        namespace.insert(p.to_string(), true);
                    }
                }
            }
            parent = p.rsplit_once('/').map(|(p, _)| p);
        }

        match namespace.get(&normalized) {
            Some(existing_is_dir) => {
                if is_dir && *existing_is_dir {
                    // A prior *explicit* directory entry with this exact
                    // path is a true duplicate; an ancestor reservation
                    // alone is not.
                    if seen_explicit_dirs.contains(&normalized) {
                        return Err(ZipIndexError::BadName(raw_name, ZipNameError::DuplicatePath));
                    }
                } else if *existing_is_dir != is_dir {
                    return Err(ZipIndexError::BadName(raw_name, ZipNameError::FileDirectoryCollision));
                } else {
                    return Err(ZipIndexError::BadName(raw_name, ZipNameError::DuplicatePath));
                }
            }
            None => {
                namespace.insert(normalized.clone(), is_dir);
            }
        }
        if is_dir {
            seen_explicit_dirs.insert(normalized.clone());
        }

        explicit_entries.push((normalized, is_dir, size, crc32, i));
    }

    let mut out: Vec<ZipIndexEntry> = Vec::with_capacity(namespace.len());
    let explicit_paths: HashSet<&str> = explicit_entries.iter().map(|(p, _, _, _, _)| p.as_str()).collect();
    for (path, is_dir, size, crc32, physical_index) in &explicit_entries {
        out.push(ZipIndexEntry {
            logical_path: path.clone(),
            is_dir: *is_dir,
            size: *size,
            crc32: *crc32,
            physical_index: *physical_index,
        });
    }
    for (path, is_dir) in &namespace {
        if *is_dir && !explicit_paths.contains(path.as_str()) {
            out.push(ZipIndexEntry {
                logical_path: path.clone(),
                is_dir: true,
                size: 0,
                crc32: 0,
                physical_index: usize::MAX,
            });
        }
    }
    Ok(out)
}

/// Streams exactly one ZIP member's bytes to `dest_file` (D-188 REJECT 5):
/// opens the archive, locates the member by its normalized logical path,
/// revalidates it against the size and CRC-32 captured at listing time —
/// rejecting a changed or replaced archive before a single byte streams
/// rather than silently extracting whatever current member now has that
/// name — then decompresses to a sibling temp file under the same limits
/// used at index time and renames into place so a reader never observes a
/// partial file. Never touches any other member.
/// Finds `internal` by the same encrypted/disallowed-mode-skipping,
/// fresh-namespace scan every ZIP read uses, then checks the decoded entry
/// against the same claimed-size/ratio/method rules applied at listing time
/// plus the caller's expected size/CRC (D-188 REJECT 5: catches an archive
/// changed since it was listed). Returns the entry's index so the caller can
/// stream it without a second scan.
/// A collision-proof name for a transaction-owned file this call will create
/// with `create_new` (D-188 REJECT 4/5): pid plus a process-wide counter, not
/// a predictable `{pid}-{i}` — two transactions racing in the same process
/// never pick the same name, and `create_new`'s `O_EXCL` semantics refuse any
/// name an unrelated process happens to already hold.
fn next_zip_stage_name(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(".{prefix}-wf-zip-{}-{n}", std::process::id())
}

/// Re-locates and fully revalidates `member` against the live archive
/// (D-188 REJECT 5): re-opens the archive, re-indexes its whole namespace
/// with the same order-independent, duplicate-detecting pass every listing
/// uses (`index_zip`, not a scan that resets its seen-sets per entry and
/// stops at the first name match), and checks archive identity (byte length)
/// plus exact entry identity (central-directory position) alongside the
/// size/CRC captured at listing time. A replacement archive that happens to
/// preserve one member's name, size and CRC at a different position is
/// caught by the position check even though the size/CRC check alone would
/// pass it. Returns an archive handle open on the revalidated file plus the
/// physical index to stream from, so the caller never re-scans.
fn locate_and_validate_zip_member(
    archive_path: &Path,
    member: &ZipMemberRef,
    cancelled: &AtomicBool,
) -> Result<(zip::ZipArchive<std::fs::File>, usize), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| format!("could not open archive: {e}"))?;
    let archive_len = file.metadata().map_err(|e| format!("could not read archive metadata: {e}"))?.len();
    if archive_len != member.archive_len {
        return Err(format!(
            "\"{}\" no longer matches what was listed — the archive changed since it was browsed",
            member.internal
        ));
    }
    let index = index_zip(file, &effective_zip_limits(), cancelled)
        .map_err(|e| format!("could not re-read the archive: {e}"))?;
    let entry = index
        .iter()
        .find(|e| e.logical_path == member.internal)
        .ok_or_else(|| format!("\"{}\" was not found in the archive", member.internal))?;
    if entry.is_dir
        || entry.size != member.size
        || entry.crc32 != member.crc32
        || entry.physical_index != member.member_index
    {
        return Err(format!(
            "\"{}\" no longer matches what was listed — the archive changed since it was browsed",
            member.internal
        ));
    }
    let physical_index = entry.physical_index;

    let reopened = std::fs::File::open(archive_path).map_err(|e| format!("could not open archive: {e}"))?;
    let mut archive = zip::ZipArchive::new(reopened).map_err(|e| format!("could not open archive: {e}"))?;
    {
        let raw = archive
            .by_index(physical_index)
            .map_err(|e| format!("could not read \"{}\": {e}", member.internal))?;
        if raw.size() > DEFAULT_ZIP_LIMITS.max_entry_claimed_bytes {
            return Err(format!("\"{}\" is too large", member.internal));
        }
        let compressed = raw.compressed_size().max(1);
        if raw.size() / compressed > DEFAULT_ZIP_LIMITS.max_compression_ratio {
            return Err(format!("\"{}\" has a suspicious compression ratio", member.internal));
        }
        #[allow(deprecated)]
        if matches!(raw.compression(), zip::CompressionMethod::Unsupported(_)) {
            return Err(format!("\"{}\" uses an unsupported compression method", member.internal));
        }
    }
    Ok((archive, physical_index))
}

/// Validates `member` against the live archive without writing anything
/// (D-188 REJECT 4 preflight pass: every requested member is checked before
/// any is staged).
fn validate_zip_member_matches(archive_path: &Path, member: &ZipMemberRef, cancelled: &AtomicBool) -> Result<(), String> {
    locate_and_validate_zip_member(archive_path, member, cancelled)?;
    Ok(())
}

/// Revalidates and streams `member`'s exact bytes to `out_path` (a path this
/// call owns outright, created with exclusive `create_new` ownership — never
/// the final destination, and never a truncating open that could silently
/// overwrite a pre-existing stage). Streaming is a manual chunked loop, not
/// `std::io::copy`, so it can enforce the archive's own claimed byte count
/// (D-188 REJECT 5: never write more than the entry's own size) and check
/// `cancelled` between chunks. Separated from publishing so a caller can
/// stage several members before committing any of them (D-188 REJECT 4).
/// D-188 REJECT 3: a per-chunk delay a test can switch on, so a GTK
/// main-loop tick-advancement proof does not depend on how fast the test
/// machine happens to memcpy a large member — it instead controls, exactly,
/// how long one real streaming extraction takes.
#[cfg(test)]
pub static TEST_STREAM_CHUNK_DELAY_MICROS: AtomicU64 = AtomicU64::new(0);

/// Test-only: an artificial per-entry delay in `index_zip`'s central-directory
/// scan, used to make a many-member archive's indexing pass measurably slow
/// without needing a real multi-thousand-entry fixture to scan quickly past a
/// GTK tick (D-188 #61). Zero in production; `index_zip` never reads it
/// outside a test build.
#[cfg(test)]
pub static TEST_INDEX_ENTRY_DELAY_MICROS: AtomicU64 = AtomicU64::new(0);

/// Unwind-safe RAII install for `TEST_INDEX_ENTRY_DELAY_MICROS`: captures the
/// exact prior value on construction and restores it in `Drop`, including on
/// panic/unwind, instead of a happy-path-only tail `.store(0, ...)` (D-188
/// round-wide shared-state audit).
#[cfg(test)]
pub struct TestIndexEntryDelayGuard {
    prior: u64,
}

#[cfg(test)]
impl TestIndexEntryDelayGuard {
    pub fn new(micros: u64) -> Self {
        let prior = TEST_INDEX_ENTRY_DELAY_MICROS.load(Ordering::Relaxed);
        TEST_INDEX_ENTRY_DELAY_MICROS.store(micros, Ordering::Relaxed);
        TestIndexEntryDelayGuard { prior }
    }
}

#[cfg(test)]
impl Drop for TestIndexEntryDelayGuard {
    fn drop(&mut self) {
        TEST_INDEX_ENTRY_DELAY_MICROS.store(self.prior, Ordering::Relaxed);
    }
}

fn stream_zip_member_to(archive_path: &Path, member: &ZipMemberRef, out_path: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let (mut archive, index) = locate_and_validate_zip_member(archive_path, member, cancelled)?;
    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("could not read \"{}\": {e}", member.internal))?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)
        .map_err(|e| format!("could not create {}: {e}", out_path.display()))?;

    let expected = entry.size();
    let mut written: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    let result = (|| -> Result<(), String> {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let n = entry.read(&mut buf).map_err(|e| format!("could not extract \"{}\": {e}", member.internal))?;
            if n == 0 {
                break;
            }
            written = written.saturating_add(n as u64);
            if written > expected {
                return Err(format!("\"{}\" streamed more bytes than the archive claimed", member.internal));
            }
            out.write_all(&buf[..n]).map_err(|e| format!("could not write {}: {e}", out_path.display()))?;
            #[cfg(test)]
            {
                let micros = TEST_STREAM_CHUNK_DELAY_MICROS.load(Ordering::Relaxed);
                if micros > 0 {
                    std::thread::sleep(std::time::Duration::from_micros(micros));
                }
            }
        }
        if written != expected {
            return Err(format!("\"{}\" streamed fewer bytes than the archive claimed", member.internal));
        }
        Ok(())
    })();
    drop(out);
    if let Err(e) = result {
        let _ = std::fs::remove_file(out_path);
        return Err(e);
    }
    Ok(())
}

pub fn extract_zip_member(member: &ZipMemberRef, dest_file: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let tmp_name = next_zip_stage_name(&dest_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let tmp = dest_file.with_file_name(tmp_name);
    stream_zip_member_to(&member.archive, member, &tmp, cancelled)?;
    rename_no_replace(&tmp, dest_file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not publish {}: {e}", dest_file.display())
    })
}

/// Private, per-session, mode-0700 cache folder for streamed-out ZIP members
/// (D-188 Open), under `runtime_dir` (the caller passes `$XDG_RUNTIME_DIR`,
/// or a test fixture's own override). Each call to `cache_path_for_zip_member`
/// gets its own numbered subfolder so two members with the same file name
/// (from the same or different archives) never collide.
pub fn ensure_zip_open_cache_dir(runtime_dir: &Path, session_id: &str, slot: u64) -> Result<PathBuf, String> {
    let root = runtime_dir.join("wayfinder-zip-cache").join(session_id).join(slot.to_string());
    std::fs::create_dir_all(&root).map_err(|e| format!("could not create cache folder: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("could not secure cache folder: {e}"))?;
    }
    Ok(root)
}

/// Owns exactly one `ensure_zip_open_cache_dir` slot (D-188 REJECT 3): no
/// production caller removed these roots before, so a long session leaked one
/// mode-0700 folder per Open forever. Dropping this value — on app shutdown
/// or in a test's own teardown — removes only the one root it created, never
/// a sibling slot or the shared `wayfinder-zip-cache` parent.
pub struct ZipOpenCacheRoot {
    path: PathBuf,
}

impl ZipOpenCacheRoot {
    pub fn create(runtime_dir: &Path, session_id: &str, slot: u64) -> Result<Self, String> {
        let path = ensure_zip_open_cache_dir(runtime_dir, session_id, slot)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ZipOpenCacheRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Streams one ZIP member out to a real destination folder (D-188 Copy Out):
/// the member's own file name, refusing (not overwriting) on a name clash.
/// Returns the path of every file actually created, for the caller to build
/// a reciprocal `Created` undo entry from, or rolls every created file back
/// and returns the first error.
/// D-188 REJECT 4: a validate-then-stage-then-publish transaction, so a
/// destination created after preflight (the old check-then-extract race) or
/// a predictable staging name (the old truncating-`File::create` temp) can
/// never be overwritten. Every member is revalidated against the live
/// archive first; only once all pass is anything written, each under its own
/// collision-proof stage name in `dest_dir`; only once every stage streamed
/// clean is anything published, each via the existing atomic no-replace
/// rename. A failure at any point removes only the stages and outputs this
/// call itself created — never a raced or unrelated object — and the
/// returned paths are exact receipts for Undo/Redo.
pub fn copy_zip_members_out(
    archive_path: &Path,
    members: &[(ZipMemberRef, String)],
    dest_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<String>, String> {
    copy_zip_members_out_inner(archive_path, members, dest_dir, cancelled, &|| {})
}

/// Identity of a file this transaction just created, captured before it is
/// renamed into place (D-188 REJECT 4): rollback re-stats the published path
/// and removes it only if this identity still matches, so a rollback after a
/// raced replacement never deletes someone else's object that merely landed
/// at the same name.
#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Removes `path` only if its live identity still matches what this
/// transaction captured when it created (or just published) the file —
/// never a plain remove-by-name (D-188 REJECT 4).
fn remove_if_still_owned(path: &Path, owned_identity: Option<(u64, u64)>) {
    if file_identity(path) == owned_identity && owned_identity.is_some() {
        let _ = std::fs::remove_file(path);
    }
}

/// Parses a candidate stage file's name against the exact grammar
/// `next_zip_stage_name` generates — `.{prefix}-wf-zip-{pid}-{counter}` —
/// and returns the embedded PID only on an exact match (D-188 ledger #20):
/// the leading dot, the last `-wf-zip-` marker (so a `prefix` itself
/// containing a dash never desyncs the split), and a PID/counter suffix
/// that is purely numeric on both sides. Anything else, including a name
/// that merely `contains("-wf-zip-")`, returns `None` and is left alone.
fn parse_owned_stage_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix('.')?;
    let marker = "-wf-zip-";
    let idx = rest.rfind(marker)?;
    if idx == 0 {
        return None;
    }
    let suffix = &rest[idx + marker.len()..];
    let (pid_str, counter_str) = suffix.split_once('-')?;
    if pid_str.is_empty() || counter_str.is_empty() {
        return None;
    }
    if !pid_str.bytes().all(|b| b.is_ascii_digit()) || !counter_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    pid_str.parse::<u32>().ok()
}

/// True only when `pid` is confirmed not alive (D-188 ledger #20): `kill(pid,
/// 0)` sends no signal but fails with `ESRCH` exactly when no such process
/// exists. Any other outcome, including "alive" or "exists but not ours to
/// signal" (`EPERM`), must never be treated as dead — a live owner's stage
/// file must survive.
#[cfg(unix)]
fn is_pid_confirmed_dead(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn is_pid_confirmed_dead(_pid: u32) -> bool {
    false
}

/// Best-effort removal of this one destination folder's own dead-owner stage
/// residue (D-188 ledger #20), run immediately before a Copy Out transaction
/// starts writing into `dest_dir`. Every one of four conditions must hold
/// before an entry is touched: its name exactly matches the stage grammar
/// (`parse_owned_stage_pid`); the embedded PID is confirmed dead
/// (`is_pid_confirmed_dead`); and a symlink-not-following check
/// (`symlink_metadata`) confirms it is a regular file. Non-recursive, this
/// one folder only, never a glob wider than the exact grammar. Any read,
/// stat or remove failure is silently skipped — sweep failure must never
/// fail the Copy Out it runs ahead of, and no real filename is ever logged.
fn sweep_dead_owned_zip_stage_files(dest_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dest_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = parse_owned_stage_pid(name) else { continue };
        if !is_pid_confirmed_dead(pid) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// `after_preflight` runs once every member has passed revalidation and
/// before anything is staged — the real production call passes a no-op, and
/// a test passes a closure that creates a destination file at this exact
/// point, to prove the check-then-stage-then-publish race the fail-closed
/// rename refusal must catch (D-188 REJECT 4).
fn copy_zip_members_out_inner(
    archive_path: &Path,
    members: &[(ZipMemberRef, String)],
    dest_dir: &Path,
    cancelled: &AtomicBool,
    after_preflight: &dyn Fn(),
) -> Result<Vec<String>, String> {
    for (member, _file_name) in members {
        validate_zip_member_matches(archive_path, member, cancelled)?;
    }
    after_preflight();
    sweep_dead_owned_zip_stage_files(dest_dir);

    let mut stages: Vec<(PathBuf, Option<(u64, u64)>, PathBuf)> = Vec::new();
    for (member, file_name) in members {
        let stage = dest_dir.join(next_zip_stage_name(file_name));
        if let Err(e) = stream_zip_member_to(archive_path, member, &stage, cancelled) {
            for (s, id, _) in &stages {
                remove_if_still_owned(s, *id);
            }
            let _ = std::fs::remove_file(&stage);
            return Err(e);
        }
        let identity = file_identity(&stage);
        stages.push((stage, identity, dest_dir.join(file_name)));
    }

    let mut created: Vec<(String, Option<(u64, u64)>)> = Vec::new();
    for (stage, identity, dest) in &stages {
        if let Err(e) = rename_no_replace(stage, dest) {
            for (s, id, _) in &stages {
                remove_if_still_owned(s, *id);
            }
            for (path, id) in &created {
                remove_if_still_owned(Path::new(path), *id);
            }
            return Err(format!("could not publish {}: {e}", dest.display()));
        }
        created.push((dest.to_string_lossy().into_owned(), *identity));
    }
    let created: Vec<String> = created.into_iter().map(|(path, _)| path).collect();
    Ok(created)
}

/// Test-only entry point that exposes the `after_preflight` injection point
/// (D-188 REJECT 4): lets a test land a conflicting destination file at the
/// exact moment after preflight validation completes and before anything is
/// staged, proving the real check-then-stage-then-publish race rather than
/// one simulated by writing the conflict before the call even starts.
#[cfg(test)]
pub fn copy_zip_members_out_for_test(
    archive_path: &Path,
    members: &[(ZipMemberRef, String)],
    dest_dir: &Path,
    cancelled: &AtomicBool,
    after_preflight: &dyn Fn(),
) -> Result<Vec<String>, String> {
    copy_zip_members_out_inner(archive_path, members, dest_dir, cancelled, after_preflight)
}

/// Immediate children of `internal` (the virtual directory being browsed; ""
/// for the ZIP root), folders-first, matching the real-directory sort
/// contract in `list_dir_cancellable` (D-188). Each row's `source` is a typed
/// `EntrySource::ZipMember`, so navigation/activation code matches on the
/// type instead of needing a ZIP special case for a path string.
pub fn zip_dir_children(archive: &Path, archive_len: u64, index: &[ZipIndexEntry], internal: &str) -> Vec<Entry> {
    let prefix = if internal.is_empty() { String::new() } else { format!("{internal}/") };
    let mut children: HashMap<String, (bool, u64, u32, usize)> = HashMap::new();
    for item in index {
        let Some(rest) = item.logical_path.strip_prefix(prefix.as_str()) else { continue };
        if rest.is_empty() {
            continue;
        }
        let (name, is_direct_dir) = match rest.split_once('/') {
            Some((first, _)) => (first.to_string(), true),
            None => (rest.to_string(), item.is_dir),
        };
        let existing = children.entry(name).or_insert((is_direct_dir, 0, 0, usize::MAX));
        if !is_direct_dir {
            existing.1 = item.size;
            existing.2 = item.crc32;
            existing.3 = item.physical_index;
        }
        existing.0 = existing.0 || is_direct_dir;
    }
    let mut out: Vec<Entry> = children
        .into_iter()
        .map(|(name, (is_dir, size, crc32, physical_index))| {
            let fake_path = Path::new(&name);
            let child_internal = if internal.is_empty() { name.clone() } else { format!("{internal}/{name}") };
            let source = EntrySource::zip_member(archive.to_path_buf(), child_internal, size, crc32, archive_len, physical_index);
            Entry {
                icon: icon_name(fake_path, is_dir).to_string(),
                is_image: is_image(fake_path),
                name,
                source,
                is_dir,
                size,
                modified: 0,
                metadata_error: None,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
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

/// How a newer drive scan differs from the one the sidebar is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveChange {
    /// Every drive, every field, in the same order.
    Same,
    /// Only `space` differs on one or more drives; the rows and what their
    /// menus act on are the same.
    SpaceOnly,
    /// A drive came, went or moved, or a field a row or its menu uses changed.
    Structural,
}

/// Compares the exact values, in order. Everything but `space` counts as
/// structural, so a field added to `Drive` later is covered without a change here.
pub fn drive_change(shown: &[Drive], scanned: &[Drive]) -> DriveChange {
    if shown == scanned {
        return DriveChange::Same;
    }
    let apart_from_space = |drive: &Drive| Drive { space: None, ..drive.clone() };
    if shown.len() == scanned.len()
        && shown.iter().zip(scanned).all(|(a, b)| apart_from_space(a) == apart_from_space(b))
    {
        DriveChange::SpaceOnly
    } else {
        DriveChange::Structural
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

/// D-265: the path a launch points at, wherever it sits on the command line.
/// `--select <path>` and `--select=<path>` name it outright, which is the line
/// an agent or a script uses to open a folder with one file already on the
/// blue bar. Otherwise the first argument that is not a flag is taken, so the
/// path no longer has to come first: `wayfinder --new-window FILE` works as
/// well as `wayfinder FILE`. Anything after a bare `--` is a path, never a flag.
pub fn launch_path_in_args(args: &[String]) -> Option<String> {
    let mut rest = args.iter();
    let mut positional: Option<String> = None;
    while let Some(arg) = rest.next() {
        if let Some(value) = arg.strip_prefix("--select=") {
            if !value.is_empty() {
                return Some(arg_to_path(value));
            }
        } else if arg == "--select" {
            if let Some(value) = rest.next() {
                if !value.is_empty() {
                    return Some(arg_to_path(value));
                }
            }
        } else if arg == "--" {
            if positional.is_none() {
                positional = rest.next().map(|value| arg_to_path(value));
            }
            break;
        } else if !arg.starts_with('-') && positional.is_none() {
            positional = Some(arg_to_path(arg));
        }
    }
    positional
}

/// D-328: `--select` adds this as one extra file argument. It is not a
/// real path; the window that receives the launch (this process or one
/// already running) reads it as "show the first path in its parent".
pub const SELECT_MARKER_SCHEME: &str = "x-wayfinder-select";
pub const SELECT_MARKER_URI: &str = "x-wayfinder-select://marker";

/// Where a `--select` launch goes.
#[derive(Debug, PartialEq, Eq)]
pub struct SelectLaunch {
    /// The folder whose window opens. Always an existing folder.
    pub folder: String,
    /// The item to highlight in it, when it exists.
    pub select: Option<String>,
}

/// D-328: the folder to open and the item to highlight for `--select path`.
/// An existing file or folder opens its parent with it highlighted. A path
/// that does not exist opens the nearest ancestor that does, highlighting
/// nothing. The filesystem root has no parent and opens itself.
pub fn select_launch_target(path: &str) -> SelectLaunch {
    let target = Path::new(path);
    if target.symlink_metadata().is_ok() {
        return match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() && parent.is_dir() => SelectLaunch {
                folder: parent.to_string_lossy().to_string(),
                select: Some(target.to_string_lossy().to_string()),
            },
            _ => SelectLaunch {
                folder: if target.is_dir() { path.to_string() } else { "/".to_string() },
                select: None,
            },
        };
    }
    let folder = target
        .ancestors()
        .skip(1)
        .find(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.is_dir())
        .map(|ancestor| ancestor.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    SelectLaunch { folder, select: None }
}

fn launch_path_from_env() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    launch_path_in_args(&args)
}

pub fn start_path_from_args() -> Option<String> {
    let arg = launch_path_from_env()?;
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
/// "show in folder" after a download, or `--select`). `start_path_from_args`
/// only yields the containing folder, so the filename would otherwise be lost.
pub fn select_path_from_args() -> Option<String> {
    let arg = launch_path_from_env()?;
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

/// One directory entry as the listing shows it, plus whether its stat failed.
fn entry_from_dirent(ent: &fs::DirEntry) -> (Entry, bool) {
    let name = ent.file_name().to_string_lossy().to_string();
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
        Err(error) => (
            ent.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
            0,
            0,
            Some(error.to_string()),
        ),
    };
    let unavailable = metadata_error.is_some();
    (
        Entry {
            name,
            source: EntrySource::real(path_buf.clone()),
            is_dir,
            size,
            modified,
            is_image: is_image(&path_buf),
            icon: icon_name(&path_buf, is_dir).to_string(),
            metadata_error,
        },
        unavailable,
    )
}

pub fn list_dir_cancellable(
    path: &Path,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<DirectoryListing, ListDirError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(ListDirError::Cancelled);
    }
    let rd = fs::read_dir(path).map_err(|err| ListDirError::Open(err.to_string()))?;
    let mut unreadable_entries = 0;
    let mut dirents = Vec::new();
    for item in rd {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ListDirError::Cancelled);
        }
        match item {
            Ok(entry) => {
                if show_hidden || !entry.file_name().to_string_lossy().starts_with('.') {
                    dirents.push(entry);
                }
            }
            Err(_) => unreadable_entries += 1,
        }
    }
    // Stat every entry. Big folders are split across a few threads; the
    // chunks are joined in order, so the result is identical to a serial pass.
    let stat_chunk = |chunk: &[fs::DirEntry]| -> Result<Vec<(Entry, bool)>, ListDirError> {
        let mut part = Vec::with_capacity(chunk.len());
        for ent in chunk {
            if cancelled.load(Ordering::Relaxed) {
                return Err(ListDirError::Cancelled);
            }
            part.push(entry_from_dirent(ent));
        }
        Ok(part)
    };
    const PARALLEL_MIN: usize = 2048;
    let threads = if dirents.len() >= PARALLEL_MIN {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(4)
    } else {
        1
    };
    let mut stat_parts: Vec<Result<Vec<(Entry, bool)>, ListDirError>> = Vec::new();
    if threads <= 1 {
        stat_parts.push(stat_chunk(&dirents));
    } else {
        let chunk_len = dirents.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let handles: Vec<_> = dirents
                .chunks(chunk_len)
                .map(|chunk| scope.spawn(|| stat_chunk(chunk)))
                .collect();
            for handle in handles {
                stat_parts.push(handle.join().unwrap_or(Err(ListDirError::Cancelled)));
            }
        });
    }
    let mut out = Vec::with_capacity(dirents.len());
    let mut metadata_unavailable = 0;
    for part in stat_parts {
        for (entry, unavailable) in part? {
            metadata_unavailable += usize::from(unavailable);
            out.push(entry);
        }
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

/// Same contract as `list_dir_cancellable`, for a ZIP virtual location:
/// re-indexes the archive's central directory (no extraction) and returns
/// that directory's immediate children. Callers dispatch on
/// `Location::ZipDir { archive, internal }` directly, never a re-parsed
/// sentinel string. Shares the generation/tab-id staleness guard applied by
/// the caller, same as a real folder (D-188).
pub fn list_zip_dir_cancellable(
    archive: &Path,
    internal: &str,
    cancelled: &AtomicBool,
) -> Result<DirectoryListing, ListDirError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(ListDirError::Cancelled);
    }
    let file = fs::File::open(archive).map_err(|e| ListDirError::Open(e.to_string()))?;
    let archive_len = file.metadata().map_err(|e| ListDirError::Open(e.to_string()))?.len();
    let index = index_zip(file, &effective_zip_limits(), cancelled).map_err(|e| match e {
        ZipIndexError::Cancelled => ListDirError::Cancelled,
        other => ListDirError::Open(other.to_string()),
    })?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(ListDirError::Cancelled);
    }
    let entries = zip_dir_children(archive, archive_len, &index, internal);
    Ok(DirectoryListing { entries, metadata_unavailable: 0, unreadable_entries: 0 })
}

/// What a search walked away with, and why it stopped early if it did.
pub struct SearchReport {
    pub entries: Vec<Entry>,
    /// Plain words for the status bar when the search did not finish, or None.
    pub stopped: Option<String>,
}

/// How long a search may run before it gives back what it has. Searching from
/// `/` with no limit looked exactly like a broken button: the status bar said
/// "Searching / ..." and nothing ever replaced it (9 November 2026).
const SEARCH_SECONDS: u64 = 25;

/// Filesystems a search must never walk into.
///
/// `/proc` and `/sys` are the kernel answering questions, not files, and they
/// are effectively bottomless. A FUSE or network mount answers every single
/// `read_dir` over a pipe or a wire -- the 4 TB Google Drive mount alone is
/// more than a search from `/` will ever finish. None of this is skipped when
/// it is the folder you are standing in: go into the drive and search there
/// and it is searched.
fn is_unsearchable_fstype(fstype: &str) -> bool {
    // `fuse.<helper>` is a program answering for a filesystem -- rclone for
    // Google Drive, gvfsd, the portal, kio-fuse, gocryptfs. `fuseblk` is NOT
    // one of these: it is how the kernel mounts an ordinary NTFS disk, and
    // Gaming and Storage are both mounted that way. Skipping anything simply
    // starting with "fuse" would have dropped two real hard drives out of
    // every search (caught 9 November 2026, before it shipped).
    is_remote_fstype(fstype)
        || fstype == GVFS_FSTYPE
        || fstype.starts_with("fuse.")
        || matches!(
            fstype,
            "proc"
                | "sysfs"
                | "devtmpfs"
                | "devpts"
                | "cgroup"
                | "cgroup2"
                | "securityfs"
                | "debugfs"
                | "tracefs"
                | "pstore"
                | "bpf"
                | "configfs"
                | "mqueue"
                | "hugetlbfs"
                | "autofs"
                | "binfmt_misc"
                | "efivarfs"
                | "fusectl"
                | "nsfs"
                | "ramfs"
                | "selinuxfs"
        )
}

/// Mount points of everything `is_unsearchable_fstype` names, read from the
/// kernel's own list so nothing is guessed from a path.
pub fn unsearchable_mounts_from(mounts: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let mountpoint = decode_mount_field(fields[1]);
        if !mountpoint.starts_with('/') {
            continue;
        }
        if is_unsearchable_fstype(fields[2]) {
            out.push(mountpoint);
        }
    }
    out
}

fn unsearchable_mounts() -> Vec<PathBuf> {
    fs::read_to_string("/proc/self/mounts")
        .map(|text| {
            unsearchable_mounts_from(&text)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn search_dir(
    root: &str,
    query: &str,
    content: bool,
    show_hidden: bool,
) -> Result<Vec<Entry>, String> {
    search_dir_report(root, query, content, show_hidden).map(|report| report.entries)
}

pub fn search_dir_report(
    root: &str,
    query: &str,
    content: bool,
    show_hidden: bool,
) -> Result<SearchReport, String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(SearchReport { entries: vec![], stopped: None });
    }

    const MAX_RESULTS: usize = 2000;
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_secs(SEARCH_SECONDS);
    let root_path = PathBuf::from(root);
    // Only mounts the search would descend INTO are skipped. One you are
    // already standing in is the thing you asked to search.
    let skip: Vec<PathBuf> = unsearchable_mounts()
        .into_iter()
        .filter(|m| *m != root_path && !root_path.starts_with(m))
        .collect();
    let mut stopped = None;
    let mut out = Vec::new();
    let mut stack = vec![root_path];

    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_RESULTS {
            stopped = Some(format!("stopped at the first {MAX_RESULTS} matches"));
            break;
        }
        if started.elapsed() >= deadline {
            stopped = Some(format!("stopped after {SEARCH_SECONDS} seconds"));
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
                    source: EntrySource::real(path.clone()),
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
                    stopped = Some(format!("stopped at the first {MAX_RESULTS} matches"));
                    break;
                }
            }
            if is_dir && !is_symlink && !skip.iter().any(|m| *m == path) {
                stack.push(path);
            }
        }
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(SearchReport { entries: out, stopped })
}

/// Remote filesystem kinds shown as network rows. Portal, AppImage, archive,
/// encrypted-overlay and other local FUSE mounts are deliberately not listed.
fn is_remote_fstype(fstype: &str) -> bool {
    matches!(
        fstype,
        "nfs"
            | "nfs4"
            | "cifs"
                | "smb3"
                | "smbfs"
                | "davfs"
                | "fuse.sshfs"
                | "fuse.davfs2"
                | "fuse.curlftpfs"
                | "fuse.smbnetfs"
                | "fuse.s3fs"
                | "fuse.gcsfuse"
    )
}

/// The desktop's runtime FUSE root whose child directories are the real shares.
const GVFS_FSTYPE: &str = "fuse.gvfsd-fuse";

/// Kernel mount-field escapes (`\040` space, `\011` tab, `\134` backslash, any
/// `\NNN` octal) back to the real path.
fn decode_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') as u32 * 64
                + (bytes[i + 2] - b'0') as u32 * 8
                + (bytes[i + 3] - b'0') as u32;
            if value < 256 {
                out.push(value as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A label with control characters removed and credentials never included.
fn clean_label(text: &str) -> String {
    let text = text.rsplit('@').next().unwrap_or(text);
    text.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// `Media on NAS` from a mount source such as `//nas/Media` or `nas:/export/Media`;
/// user names and passwords in the source are dropped, never shown.
fn label_from_source(source: &str) -> Option<String> {
    let (host, share) = if let Some(rest) = source.strip_prefix("//") {
        let (host, path) = rest.split_once('/')?;
        (host, path)
    } else {
        source.split_once(':')?
    };
    let host = clean_label(host);
    let share = clean_label(share.trim_matches('/').rsplit('/').next().unwrap_or(""));
    match (share.is_empty(), host.is_empty()) {
        (false, false) => Some(format!("{share} on {host}")),
        (true, false) => Some(host),
        _ => None,
    }
}

/// A GVfs child directory name (`smb-share:server=nas,share=media`,
/// `sftp:host=example.com,user=bob`) as a plain label. Only server, share and
/// host are read; `user`, `password` and everything else is ignored.
fn label_from_gvfs_child(name: &str) -> String {
    let (scheme, params) = name.split_once(':').unwrap_or((name, ""));
    let get = |key: &str| {
        params
            .split(',')
            .filter_map(|part| part.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| clean_label(v))
            .filter(|v| !v.is_empty())
    };
    let friendly = match (get("share"), get("server").or_else(|| get("host"))) {
        (Some(share), Some(host)) => Some(format!("{share} on {host}")),
        (None, Some(host)) => Some(host),
        _ => None,
    };
    friendly.unwrap_or_else(|| {
        let plain = clean_label(scheme);
        if plain.is_empty() || params.contains("password") {
            "Network share".to_string()
        } else {
            plain
        }
    })
}

/// Backend identity `(host, share)` of a GVfs child named
/// `smb-share:server=nas,share=media[,user=..]`, percent-decoded. Only those
/// two keys are read; any other scheme gives `None`.
pub fn gvfs_smb_identity(name: &str) -> Option<(String, String)> {
    let (scheme, params) = name.split_once(':')?;
    if scheme != "smb-share" {
        return None;
    }
    let get = |key: &str| {
        params
            .split(',')
            .filter_map(|part| part.split_once('='))
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| percent_decode_lossless(v))
            .filter(|v| !v.is_empty())
    };
    Some((get("server")?, get("share")?))
}

fn percent_decode_lossless(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Every SMB share under every GVfs runtime root in `/proc/mounts` text, as
/// `(host, share, local path)`. Local reads only; nothing remote is opened.
pub fn gvfs_smb_shares(mounts: &str, children: &dyn Fn(&str) -> Option<Vec<String>>) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[2] != GVFS_FSTYPE {
            continue;
        }
        let root = decode_mount_field(fields[1]);
        if !root.starts_with('/') {
            continue;
        }
        for name in children(&root).unwrap_or_default() {
            if name.contains('/') {
                continue;
            }
            if let Some((host, share)) = gvfs_smb_identity(&name) {
                out.push((host, share, format!("{}/{name}", root.trim_end_matches('/'))));
            }
        }
    }
    out
}

/// The same, read from this machine right now.
pub fn gvfs_smb_shares_now() -> Vec<(String, String, String)> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    gvfs_smb_shares(&mounts, &gvfs_children)
}

/// Immediate child directories of a GVfs runtime root; a missing or unreadable
/// root is `None`, and an entry that vanishes mid-scan is skipped alone.
fn gvfs_children(root: &str) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() && !name.starts_with('.') {
            names.push(name);
        }
    }
    Some(names)
}

/// Network rows from `/proc/mounts` text: direct remote mounts anywhere, plus one
/// row per immediate child of a GVfs runtime root (never the root itself).
/// Deduplicated by decoded lexical path (no canonicalizing, no remote traversal),
/// skipping `taken`, sorted by label then path. Pure apart from `children`.
pub fn network_drives(
    mounts: &str,
    taken: &std::collections::HashSet<String>,
    children: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Vec<Drive> {
    let mut seen: std::collections::HashSet<String> = taken.clone();
    let mut rows: Vec<Drive> = Vec::new();
    let mut push = |label: String, path: String, fstype: &str| {
        if seen.insert(path.clone()) {
            rows.push(Drive {
                name: if label.is_empty() { "Network share".to_string() } else { label },
                path: path.clone(),
                mountpoint: path,
                size: 0,
                fstype: fstype.to_string(),
                kind: "network".to_string(),
                removable: false,
                space: None,
            });
        }
    };
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let (source, fstype) = (decode_mount_field(fields[0]), fields[2]);
        let mountpoint = decode_mount_field(fields[1]);
        if !mountpoint.starts_with('/') || mountpoint.contains('\0') || mountpoint.contains('\n') {
            continue;
        }
        if fstype == GVFS_FSTYPE {
            let Some(names) = children(&mountpoint) else { continue };
            for name in names {
                if name.is_empty() || name.contains('/') {
                    continue;
                }
                let path = format!("{}/{name}", mountpoint.trim_end_matches('/'));
                push(label_from_gvfs_child(&name), path, fstype);
            }
        } else if is_remote_fstype(fstype) {
            let label = label_from_source(&source).unwrap_or_else(|| {
                clean_label(mountpoint.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
            });
            push(label, mountpoint, fstype);
        }
    }
    rows.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path))
    });
    rows
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
            let (mountpoint, fstype) = (fields[1], fields[2]);
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
            }
        }
        let taken: std::collections::HashSet<String> =
            out.iter().map(|d| d.mountpoint.clone()).filter(|m| !m.is_empty()).collect();
        out.extend(network_drives(&mounts, &taken, &gvfs_children));
    }

    probe_local_space(&mut out, probe);
    out
}

/// One space probe per mounted drive, except network rows: a dead remote share
/// can block `statvfs`, so discovery never touches one and leaves its space unknown.
fn probe_local_space(drives: &mut [Drive], probe: &dyn Fn(&str) -> Option<Space>) {
    for drive in drives.iter_mut().filter(|d| !d.mountpoint.is_empty() && d.kind != "network") {
        drive.space = probe(&drive.mountpoint);
    }
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
    let text = String::from_utf8_lossy(&buf).into_owned();
    // D-240: what is read and what is shown are different sizes. The preview
    // is one GtkLabel, and Pango lays out and measures every character in it
    // before the window can draw again, so handing it a quarter of a megabyte
    // froze the whole app for many seconds when a large plain-text file was
    // selected -- the Blender manual exported as .txt is what found this.
    // Reading 256 KB is still right (it is what the binary check needs), but
    // only the first part of it is displayed. A preview is there to say what
    // the file is, not to read it in.
    let shown = preview_cut(&text);
    let truncated = shown.len() < text.len() || md.len() as usize > n;
    let mut out = shown.to_string();
    if truncated {
        out.push_str("\n\n... (preview truncated)");
    }
    Ok(out)
}

/// The longest slice of `text` a preview label is allowed to hold: at most
/// `PREVIEW_SHOW_LINES` lines and `PREVIEW_SHOW_BYTES` bytes, always ending on
/// a character boundary so the string stays valid UTF-8.
fn preview_cut(text: &str) -> &str {
    const PREVIEW_SHOW_BYTES: usize = 16 * 1024;
    const PREVIEW_SHOW_LINES: usize = 300;

    let mut end = text.len().min(PREVIEW_SHOW_BYTES);
    while end < text.len() && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut lines = 0;
    for (index, byte) in text.as_bytes().iter().enumerate().take(end) {
        if *byte == b'\n' {
            lines += 1;
            if lines == PREVIEW_SHOW_LINES {
                end = index;
                break;
            }
        }
    }
    &text[..end]
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

/// D-340: the Windows answer to a rename onto a taken name: the first free
/// "stem (n).ext" in the same folder, n from 2. A folder keeps its whole name
/// as the stem, so "a.b" becomes "a.b (2)".
pub fn free_sibling_name(parent: &Path, name: &str, is_dir: bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|cand| parent.join(cand).symlink_metadata().is_err())
        .expect("a free name exists")
}

/// D-359: a path that Wayfinder itself renamed, moved or trashed. `(from, Some(to))`
/// is a rename or move, `(from, None)` a removal. The app installs one hook so
/// settings that name paths (pins) can follow the item; with none installed this
/// does nothing, so the core stays usable and testable without the app.
pub type PathChangeHook = fn(&[(String, Option<String>)]);
static PATH_CHANGE_HOOK: std::sync::OnceLock<PathChangeHook> = std::sync::OnceLock::new();

pub fn set_path_change_hook(hook: PathChangeHook) {
    let _ = PATH_CHANGE_HOOK.set(hook);
}

fn notify_path_changes(changes: &[(String, Option<String>)]) {
    if let Some(hook) = PATH_CHANGE_HOOK.get() {
        if !changes.is_empty() {
            hook(changes);
        }
    }
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
    notify_path_changes(&[(path.to_string(), Some(dest.to_string_lossy().to_string()))]);
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

    let resolved: Vec<ResolvedItem> = operations
        .into_iter()
        .map(|(from, dest)| ResolvedItem {
            from: from.to_string_lossy().to_string(),
            to: dest.to_string_lossy().to_string(),
        })
        .collect();
    notify_path_changes(&resolved.iter().map(|i| (i.from.clone(), Some(i.to.clone()))).collect::<Vec<_>>());
    Ok(resolved)
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
        notify_path_changes(&[(path.clone(), None)]);
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

/// D-187: the exact `.trashinfo`/payload identity a single `trash_paths_exact`
/// call created for one source. Restoring by this selector (a direct
/// `files/<stem>` path) can never pick the wrong entry when two items shared
/// one original path and were trashed separately — unlike `restore_from_trash`,
/// which picks "newest `.trashinfo` for this original path" and is therefore
/// wrong once a second item with the same original path lands in Trash after
/// the first.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashReceipt {
    pub original: String,
    pub selector: String,
}

/// `trash_paths_exact` identifies its new receipt by diffing the real
/// `Trash/files` directory before/after; any other test trashing something to
/// the SAME real Trash at the same moment makes that diff ambiguous. Every
/// test across the crate that calls `trash_paths_exact` on the real Trash
/// must hold this for its whole run, serializing those tests against each
/// other under cargo's parallel runner.
#[cfg(test)]
pub static REAL_TRASH_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// D-231: a Trash of a test's own. While one is installed, the exact batch's
/// move and its Trash folder are this fixture's, not GIO's and the home Trash,
/// so the test never reads or changes the real Trash. The move is a real one:
/// the `.trashinfo` is written, then the item is renamed into `<dir>/files`,
/// the way the real Trash does it. A fault is asked for by the item's file
/// name. Installing holds REAL_TRASH_TEST_LOCK, so no real-Trash test runs
/// meanwhile.
#[cfg(test)]
pub mod fixture_trash {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    #[derive(Default)]
    struct Active {
        dir: PathBuf,
        refuse: Vec<OsString>,
        twin: Vec<OsString>,
        blind_after: Vec<OsString>,
        asked: Vec<String>,
    }

    static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

    fn active() -> MutexGuard<'static, Option<Active>> {
        ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The fixture stays installed for as long as this lives.
    pub struct Installed {
        _real_trash: MutexGuard<'static, ()>,
    }

    pub fn install(dir: &Path) -> Installed {
        let real_trash = super::REAL_TRASH_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *active() = Some(Active { dir: dir.to_path_buf(), ..Default::default() });
        Installed { _real_trash: real_trash }
    }

    impl Installed {
        fn with<T>(&self, change: impl FnOnce(&mut Active) -> T) -> T {
            change(active().as_mut().expect("the fixture Trash is installed"))
        }

        /// The move of an item with this file name fails and leaves it in place.
        pub fn refuse(&self, name: &str) {
            self.with(|a| a.refuse.push(name.into()));
        }

        /// A second new entry lands in the Trash together with this item.
        pub fn twin(&self, name: &str) {
            self.with(|a| a.twin.push(name.into()));
        }

        /// Once this item is in it, the Trash `files` folder cannot be listed.
        pub fn blind_after(&self, name: &str) {
            self.with(|a| a.blind_after.push(name.into()));
        }

        /// Takes the `files` folder's read access away, or gives it back.
        pub fn blind(&self, blind: bool) {
            let files = self.with(|a| a.dir.join("files"));
            fs::set_permissions(files, fs::Permissions::from_mode(if blind { 0o300 } else { 0o700 })).unwrap();
        }

        pub fn clear_faults(&self) {
            self.with(|a| {
                a.refuse.clear();
                a.twin.clear();
                a.blind_after.clear();
            });
        }

        /// Every path the move was asked for since the last call, in order.
        pub fn asked(&self) -> Vec<String> {
            self.with(|a| std::mem::take(&mut a.asked))
        }
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            if let Some(a) = active().take() {
                let _ = fs::set_permissions(a.dir.join("files"), fs::Permissions::from_mode(0o700));
            }
        }
    }

    pub(super) fn dir() -> Option<PathBuf> {
        active().as_ref().map(|a| a.dir.clone())
    }

    fn trashinfo(original: &str) -> String {
        let encoded: String = original
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        format!("[Trash Info]\nPath={encoded}\nDeletionDate=2026-10-05T00:00:00\n")
    }

    /// None when no fixture is installed. While one is, the answer is always
    /// the fixture's own, so nothing can fall through to the real Trash.
    pub(super) fn move_item(path: &str) -> Option<Result<(), String>> {
        let mut guard = active();
        let a = guard.as_mut()?;
        a.asked.push(path.to_string());
        let Some(name) = Path::new(path).file_name().map(|name| name.to_os_string()) else {
            return Some(Err("the fixture Trash needs a file name".to_string()));
        };
        if a.refuse.contains(&name) {
            return Some(Err("the fixture Trash refused this item".to_string()));
        }
        let (files, info) = (a.dir.join("files"), a.dir.join("info"));
        let moved = (|| -> std::io::Result<()> {
            fs::create_dir_all(&files)?;
            fs::create_dir_all(&info)?;
            // The first free stem, the way the real Trash numbers a taken name.
            let mut stem = name.clone();
            let mut n = 1;
            while fs::symlink_metadata(files.join(&stem)).is_ok() {
                n += 1;
                stem = format!("{}.{n}", name.to_string_lossy()).into();
            }
            let info_file = info.join(format!("{}.trashinfo", stem.to_string_lossy()));
            fs::write(&info_file, trashinfo(path))?;
            if let Err(err) = fs::rename(path, files.join(&stem)) {
                let _ = fs::remove_file(&info_file);
                return Err(err);
            }
            if a.twin.contains(&name) {
                let twin = format!("{}.twin", stem.to_string_lossy());
                fs::write(info.join(format!("{twin}.trashinfo")), trashinfo("/someone/elses/item"))?;
                fs::write(files.join(&twin), "someone else's item")?;
            }
            if a.blind_after.contains(&name) {
                fs::set_permissions(&files, fs::Permissions::from_mode(0o300))?;
            }
            Ok(())
        })();
        Some(moved.map_err(|err| err.to_string()))
    }
}

/// The Trash folder the exact batch and the exact restore work in.
fn trash_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(dir) = fixture_trash::dir() {
        return Ok(dir);
    }
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    Ok(Path::new(&home).join(".local/share/Trash"))
}

/// D-271: Trash is per filesystem, not per user. GIO moves an item on another
/// volume into that volume's own `.Trash-1000`, never into the home Trash, so
/// listing the home Trash before and after the move saw no new entry and the
/// delete was reported as failed although the item had moved (16 November 2026,
/// the owner: "am not able to delete from the g drive on way finder" — the files
/// were in `GoogleDrive/.Trash-1000` the whole time). This resolves the Trash
/// that will actually receive `path`.
fn trash_dir_for(path: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let home_trash = trash_dir()?;
    #[cfg(test)]
    if fixture_trash::dir().is_some() {
        return Ok(home_trash);
    }
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let Some(mount) = mount_root_of(Path::new(path)) else {
        return Ok(home_trash);
    };
    let home_mount = mount_root_of(Path::new(&home));
    if home_mount.as_deref() == Some(mount.as_path()) {
        return Ok(home_trash);
    }
    let uid = fs::metadata(&home).map(|m| m.uid()).unwrap_or(0);
    let top = mount.join(format!(".Trash-{uid}"));
    if top.is_dir() {
        return Ok(top);
    }
    let shared = mount.join(".Trash").join(uid.to_string());
    if shared.is_dir() {
        return Ok(shared);
    }
    // Neither exists yet; GIO creates the top-level one on the first move.
    Ok(top)
}

/// The directory where `path`'s filesystem is mounted: the highest ancestor
/// still on the same device. `None` if nothing about `path` can be read.
fn mount_root_of(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut current = path;
    let dev = loop {
        match fs::symlink_metadata(current) {
            Ok(meta) => break meta.dev(),
            Err(_) => current = current.parent()?,
        }
    };
    let mut root = current.to_path_buf();
    while let Some(parent) = root.parent() {
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.dev() == dev => root = parent.to_path_buf(),
            _ => break,
        }
    }
    Some(root)
}

/// The one call that moves an item to Trash. The reason comes back bare.
fn trash_move(path: &str) -> Result<(), String> {
    #[cfg(test)]
    if let Some(moved) = fixture_trash::move_item(path) {
        return moved;
    }
    gio::File::for_path(path)
        .trash(gio::Cancellable::NONE)
        .map_err(|err| err.to_string())
}

/// D-231: one item the exact batch moved: where it now sits in Trash, and the
/// content guard taken from the item itself just before its move.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashedItem {
    pub receipt: TrashReceipt,
    pub fingerprint: Fingerprint,
}

/// D-231: the item an exact batch stopped at, and the state it was left in.
#[derive(Clone, Debug, PartialEq)]
pub enum TrashStop {
    /// The item is where it was; nothing about it changed.
    NotMoved { path: String, reason: String },
    /// The item is in Trash, but its entry there could not be told apart from
    /// any other, so it has no receipt and no Undo.
    MovedUnidentified { path: String, reason: String },
}

/// D-231: what one exact batch did, in the order it was asked. The moved items
/// are kept whatever happened to a later one.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashBatch {
    pub done: Vec<TrashedItem>,
    pub stopped: Option<TrashStop>,
    pub not_attempted: Vec<String>,
}

/// The entry names in `Trash/files`. A folder or an entry that cannot be read
/// is an error, never an empty answer.
fn files_dir_stems(files_dir: &Path) -> std::io::Result<HashSet<std::ffi::OsString>> {
    fs::read_dir(files_dir)?.map(|entry| entry.map(|entry| entry.file_name())).collect()
}

/// Trashes each path one at a time and stops at the first one that fails;
/// the items before it stay recorded and the items after it are not tried.
pub fn trash_paths_exact(paths: &[String]) -> TrashBatch {
    let mut batch = TrashBatch { done: Vec::with_capacity(paths.len()), stopped: None, not_attempted: Vec::new() };
    for (index, path) in paths.iter().enumerate() {
        match trash_one_exact(path) {
            Ok(item) => {
                notify_path_changes(&[(path.clone(), None)]);
                batch.done.push(item)
            }
            Err(stop) => {
                // A move that happened but could not be identified still left the path.
                if let TrashStop::MovedUnidentified { path, .. } = &stop {
                    notify_path_changes(&[(path.clone(), None)]);
                }
                batch.stopped = Some(stop);
                batch.not_attempted = paths[index + 1..].to_vec();
                break;
            }
        }
    }
    batch
}

/// One item: its content guard first, then the move, with `Trash/files` listed
/// before and after to capture the exact stem the move gave it. Anything that
/// fails before the move leaves the item untouched. After the move, a listing
/// that fails or does not show exactly one new entry is reported as moved but
/// not identified, rather than guessing which entry it is.
fn trash_one_exact(path: &str) -> Result<TrashedItem, TrashStop> {
    let not_moved = |reason: String| TrashStop::NotMoved { path: path.to_string(), reason };
    let unidentified = |reason: String| TrashStop::MovedUnidentified { path: path.to_string(), reason };
    let unlisted = |err: std::io::Error| format!("cannot read the Trash folder: {err}");
    let files_dir = trash_dir_for(path).map_err(not_moved)?.join("files");
    let fingerprint = fingerprint(path)
        .map_err(|err| not_moved(err.strip_prefix(&format!("{path}: ")).unwrap_or(&err).to_string()))?;
    let before = match files_dir_stems(&files_dir) {
        Ok(stems) => stems,
        // A Trash that has never held anything: the first move makes the folder.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(err) => return Err(not_moved(unlisted(err))),
    };
    trash_move(path).map_err(not_moved)?;
    let after = files_dir_stems(&files_dir).map_err(|err| unidentified(unlisted(err)))?;
    let new: Vec<_> = after.difference(&before).collect();
    let [stem] = new[..] else {
        return Err(unidentified(format!("{} new Trash entries, expected 1", new.len())));
    };
    let selector = files_dir.join(stem).to_string_lossy().to_string();
    Ok(TrashedItem { receipt: TrashReceipt { original: path.to_string(), selector }, fingerprint })
}

/// Restores each receipt's exact payload, never "the newest item for this
/// original path". All-or-nothing: on a mid-batch failure, every receipt
/// already restored in this call is put back in Trash under the same
/// selector before returning the error.
pub fn restore_from_trash_exact(receipts: &[TrashReceipt]) -> Result<(), String> {
    let home_trash = trash_dir()?;
    let mut restored = Vec::new();
    for receipt in receipts {
        // D-271: the receipt's selector is `<trash>/files/<stem>`, so it names
        // the Trash the item actually went to — which is the volume's own when
        // the item was not on the home filesystem.
        let trash = Path::new(&receipt.selector)
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home_trash.clone());
        match restore_from_trash_in(&trash, &receipt.selector) {
            Ok(()) => restored.push(receipt),
            Err(err) => {
                for done in restored.into_iter().rev() {
                    let _ = trash_paths_exact(std::slice::from_ref(&done.original));
                }
                return Err(err);
            }
        }
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
    rename_no_replace_io(from, to).map_err(|err| match err.raw_os_error() {
        Some(libc::EEXIST) => format!("already exists: {}", to.display()),
        Some(_) => format!("{}: {err}", to.display()),
        None => err.to_string(),
    })
}

/// The same move with the system's own error kept, so a caller can tell a taken
/// name (`EEXIST`) from a real failure without reading message text (D-230).
fn rename_no_replace_io(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path contains a NUL byte: {}", path.display()),
            )
        })
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
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
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
    // The folder an item came from is often long gone by the time it is
    // restored (D-225: a scratch folder deleted weeks later). Refusing left the
    // item stranded in the Trash with no way out, so the path is rebuilt first.
    // A path blocked by a FILE of that name is still a refusal, not a replace.
    if !parent.is_dir() {
        if parent.symlink_metadata().is_ok() {
            return Err(format!(
                "destination folder is blocked by a file: {}",
                parent.display()
            ));
        }
        fs::create_dir_all(parent).map_err(|err| {
            format!("cannot recreate destination folder {}: {err}", parent.display())
        })?;
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
pub fn check_conflicts(srcs: &[PathBuf], dest_dir: &Path, is_copy: bool) -> Vec<String> {
    srcs.iter()
        .filter_map(|src| {
            let name = src.file_name()?.to_str()?.to_string();
            let target = dest_dir.join(&name);
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
pub fn is_noop_move(srcs: &[PathBuf], dest_dir: &Path, is_copy: bool) -> bool {
    !is_copy
        && !srcs.is_empty()
        && srcs.iter().all(|src| {
            let Some(name) = src.file_name() else {
                return false;
            };
            matches!(
                (fs::canonicalize(src), fs::canonicalize(dest_dir.join(name))),
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
        "add_number" => Some(PasteConflict::KeepBothPlainNumber),
        _ => None,
    }
}

/// What to do when the destination already holds a file of the same name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PasteConflict {
    /// Paste alongside, as "name (2)". Never destroys anything.
    KeepBoth,
    /// D-361: paste alongside, with a plain number on the end of the name and
    /// no space or brackets: "Sheree2.jpg", "Sheree3.jpg". Never destroys anything.
    KeepBothPlainNumber,
    /// Overwrite the existing item. DESTRUCTIVE and not undoable — the undo
    /// stack can move the pasted item back out, but whatever was overwritten is
    /// already gone.
    Replace,
}

/// Explorer drag action: Ctrl copies, Shift moves, otherwise MOVE only when
/// every source and the destination folder are on the same device. If any
/// metadata cannot be read the choice is an explicit conservative COPY.
pub fn drag_is_copy(srcs: &[PathBuf], dest_dir: &Path, ctrl: bool, shift: bool) -> bool {
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

/// The message a cancelled walk or hash returns; `Stop::from` turns it back
/// into `Stop::Cancelled` so a cancel is never mistaken for a disk failure.
const CANCELLED_MSG: &str = "transfer cancelled";

static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err(CANCELLED_MSG.to_string())
    } else {
        Ok(())
    }
}

fn hash_file_bytes(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    path: &Path,
    len: u64,
    cancel: &AtomicBool,
    tick: &dyn Fn(u64),
) -> Result<(), String> {
    use std::hash::Hasher;
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut seen = 0u64;
    loop {
        check_cancel(cancel)?;
        let got = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if got == 0 {
            break;
        }
        hasher.write(&buf[..got]);
        seen += got as u64;
        tick(got as u64);
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
    cancel: &AtomicBool,
    tick: &dyn Fn(u64),
) -> Result<(), String> {
    use std::hash::Hasher;
    let mut names = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.file_name()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    for name in names {
        check_cancel(cancel)?;
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
            hash_tree(hasher, &path, entries, cancel, tick)?;
            hasher.write_u8(b'/');
        } else {
            hasher.write_u8(b'F');
            hasher.write_u64(meta.len());
            hash_file_bytes(hasher, &path, meta.len(), cancel, tick)?;
        }
    }
    Ok(())
}

pub fn fingerprint(path: &str) -> Result<Fingerprint, String> {
    fingerprint_c(path, &NEVER_CANCELLED)
}

fn fingerprint_c(path: &str, cancel: &AtomicBool) -> Result<Fingerprint, String> {
    fingerprint_tick(path, cancel, &|_| {})
}

/// `fingerprint_c`, reporting every chunk of file bytes it reads, so a long
/// read of a slow drive can show progress instead of a silent wait.
fn fingerprint_tick(path: &str, cancel: &AtomicBool, tick: &dyn Fn(u64)) -> Result<Fingerprint, String> {
    use std::hash::Hasher;
    let p = Path::new(path);
    let meta = fs::symlink_metadata(p).map_err(|e| format!("{path}: {e}"))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if meta.file_type().is_symlink() {
        let target = fs::read_link(p).map_err(|e| e.to_string())?;
        Ok(Fingerprint::Link(target.to_string_lossy().to_string()))
    } else if meta.is_dir() {
        let mut entries = 0;
        hash_tree(&mut hasher, p, &mut entries, cancel, tick)?;
        Ok(Fingerprint::Tree { entries, digest: hasher.finish() })
    } else {
        hash_file_bytes(&mut hasher, p, meta.len(), cancel, tick)?;
        Ok(Fingerprint::File { len: meta.len(), digest: hasher.finish() })
    }
}

/// Fails if any requested source cannot be recorded.
#[cfg(any(test))]
pub fn snapshot_sources(srcs: &[String]) -> Result<Vec<SourceSnapshot>, String> {
    snapshot_sources_c(srcs, &NEVER_CANCELLED)
}

fn snapshot_sources_c(srcs: &[String], cancel: &AtomicBool) -> Result<Vec<SourceSnapshot>, String> {
    snapshot_sources_tick(srcs, cancel, &|_| {})
}

fn snapshot_sources_tick(srcs: &[String], cancel: &AtomicBool, tick: &dyn Fn(u64)) -> Result<Vec<SourceSnapshot>, String> {
    use std::os::unix::fs::MetadataExt;
    srcs.iter()
        .map(|p| {
            let meta = fs::symlink_metadata(p).map_err(|e| format!("cannot read {p}: {e}"))?;
            Ok(SourceSnapshot { path: p.clone(), ino: meta.ino(), fp: fingerprint_tick(p, cancel, tick)? })
        })
        .collect()
}

/// Fail-closed postcondition. COPY: every source still present with the same
/// identity and content, every destination present with matching content. MOVE:
/// every moved source gone and every destination present with matching content.
#[cfg(any(test))]
pub fn verify_transfer(
    before: &[SourceSnapshot],
    items: &[ResolvedItem],
    is_copy: bool,
) -> Result<(), String> {
    verify_transfer_c(before, items, is_copy, &NEVER_CANCELLED)
}

fn verify_transfer_c(
    before: &[SourceSnapshot],
    items: &[ResolvedItem],
    is_copy: bool,
    cancel: &AtomicBool,
) -> Result<(), String> {
    verify_transfer_tick(before, items, is_copy, cancel, &|_| {}).map(|_| ())
}

/// D-320: one verified top-level item of a transfer, kept so the person can
/// see exactly what was checked. `files` lists up to `REPORT_FILE_CAP` files
/// of a folder with their sizes; `files_total` is the true count.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportRow {
    pub from: String,
    pub to: String,
    /// "file", "folder" or "link".
    pub kind: &'static str,
    pub bytes: u64,
    pub files_total: u64,
    pub files: Vec<(String, u64)>,
    pub digest: String,
}

pub const REPORT_FILE_CAP: usize = 2000;

fn walk_report(dir: &Path, rel: &Path, rows: &mut Vec<(String, u64)>, total: &mut u64, bytes: &mut u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut names: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        let rel_path = rel.join(&name);
        if meta.is_dir() && !meta.file_type().is_symlink() {
            walk_report(&path, &rel_path, rows, total, bytes);
        } else {
            *total += 1;
            *bytes += meta.len();
            if rows.len() < REPORT_FILE_CAP {
                rows.push((rel_path.to_string_lossy().to_string(), meta.len()));
            }
        }
    }
}

fn report_row(item: &ResolvedItem, fp: &Fingerprint) -> ReportRow {
    let mut row = ReportRow {
        from: item.from.clone(),
        to: item.to.clone(),
        kind: "file",
        bytes: 0,
        files_total: 1,
        files: Vec::new(),
        digest: String::new(),
    };
    match fp {
        Fingerprint::File { len, digest } => {
            row.bytes = *len;
            row.digest = format!("{digest:016x}");
        }
        Fingerprint::Link(t) => {
            row.kind = "link";
            row.digest = t.clone();
        }
        Fingerprint::Tree { digest, .. } => {
            row.kind = "folder";
            row.digest = format!("{digest:016x}");
            row.files_total = 0;
            walk_report(Path::new(&item.to), Path::new(""), &mut row.files, &mut row.files_total, &mut row.bytes);
        }
    }
    row
}

fn verify_transfer_tick(
    before: &[SourceSnapshot],
    items: &[ResolvedItem],
    is_copy: bool,
    cancel: &AtomicBool,
    tick: &dyn Fn(u64),
) -> Result<Vec<ReportRow>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut report = Vec::with_capacity(items.len());
    for item in items {
        let Some(src) = before.iter().find(|s| s.path == item.from) else {
            return Err(format!("transfer check: no record of {}", item.from));
        };
        let dest = fingerprint_tick(&item.to, cancel, tick).map_err(|e| {
            if e == CANCELLED_MSG {
                e
            } else {
                format!("transfer check: {} is missing after the transfer", item.to)
            }
        })?;
        if dest != src.fp {
            return Err(format!("transfer check: {} does not match its source", item.to));
        }
        let now = fs::symlink_metadata(&item.from);
        if is_copy {
            let now = now
                .map_err(|_| format!("transfer check: copy removed the source {}", item.from))?;
            if now.ino() != src.ino || fingerprint_tick(&item.from, cancel, tick)? != src.fp {
                return Err(format!("transfer check: copy changed the source {}", item.from));
            }
        } else if now.is_ok() {
            return Err(format!("transfer check: move left the source {}", item.from));
        }
        report.push(report_row(item, &dest));
    }
    Ok(report)
}

/// Result of a committed transfer. `warnings` are cleanup problems after a valid,
/// verified commit: the visible end state is exact and Undo applies.
#[derive(Debug)]
pub struct TransferResult {
    pub items: Vec<ResolvedItem>,
    pub warnings: Vec<String>,
    /// D-320: what the final check verified, one row per top-level item.
    pub report: Vec<ReportRow>,
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

/// What a running transfer reports. `Bytes` is logical regular-file bytes for
/// the whole batch: `done <= total`, and `done` never goes back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransferEvent {
    Preparing,
    /// Reading the sources once to record what they hold, before any copy
    /// starts. On a slow drive this is the long wait; `done <= total`.
    Reading { done: u64, total: u64 },
    Bytes { done: u64, total: u64 },
    /// An item (by position in the request) is about to be staged: its byte
    /// size, so a window can show which item is copying and how far along.
    Item { index: usize, bytes: u64 },
    Verifying,
    /// Hashing the copy and the result to prove they match: how many bytes of
    /// the bytes to hash are done. `done <= total`.
    Checking { done: u64, total: u64 },
    /// The verified commit point: Cancel no longer applies.
    Committing,
}

#[derive(Debug, PartialEq)]
pub enum TransferFail {
    /// Stopped on request and rolled back exactly.
    Cancelled,
    Failed(String),
}

/// Inside a transfer a cancel and a disk failure are different outcomes.
enum Stop {
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        if message == CANCELLED_MSG {
            Stop::Cancelled
        } else {
            Stop::Failed(message)
        }
    }
}

impl From<&str> for Stop {
    fn from(message: &str) -> Self {
        Stop::from(message.to_string())
    }
}

/// The worker's handle: the cancel token, the progress sink, and the program
/// that stages a copy (`cp`; a test may name a slower stand-in).
pub struct TransferCtl<'a> {
    pub cancel: &'a AtomicBool,
    pub on_event: &'a dyn Fn(TransferEvent),
    pub copy_program: &'a str,
}

/// Batch byte totals never wrap: too large is a specific failure.
fn add_bytes(total: u64, more: u64) -> Result<u64, String> {
    total
        .checked_add(more)
        .ok_or_else(|| "the items are too large to count".to_string())
}

/// Logical regular-file bytes under `path`: symlinks are not followed and
/// directory entries carry no bytes.
fn count_bytes(path: &Path, cancel: &AtomicBool) -> Result<u64, String> {
    check_cancel(cancel)?;
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        Ok(0)
    } else if meta.is_dir() {
        let mut sum = 0;
        for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            sum = add_bytes(sum, count_bytes(&entry.path(), cancel)?)?;
        }
        Ok(sum)
    } else {
        Ok(meta.len())
    }
}

/// Best-effort bytes already written under a growing stage (same measure as
/// `count_bytes`; a read error just counts as nothing yet).
fn staged_bytes(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => 0,
        Ok(meta) if meta.is_dir() => fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| staged_bytes(&e.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

/// Monotonic byte reporting: emits only a strictly larger `done`, never above
/// `total`.
struct Progress<'a> {
    total: u64,
    last: Option<u64>,
    on_event: &'a dyn Fn(TransferEvent),
}

impl Progress<'_> {
    fn advance(&mut self, done: u64) {
        let done = done.min(self.total);
        if self.last.is_none_or(|last| done > last) {
            self.last = Some(done);
            (self.on_event)(TransferEvent::Bytes { done, total: self.total });
        }
    }
}

/// Stages one item with the exact `cp -a -T` child, which is polled: a cancel
/// kills and reaps that child only, and bytes come from what it has written.
fn copy_item(
    ctl: &TransferCtl,
    src: &str,
    stage: &Path,
    base: u64,
    item_bytes: u64,
    progress: &mut Progress,
) -> Result<(), Stop> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = Command::new(ctl.copy_program)
        .args(["-a", "-T", "--", src])
        .arg(stage)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", ctl.copy_program))?;
    let mut stderr = child.stderr.take();
    let drain = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let mut next_measure = Instant::now();
    loop {
        if ctl.cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            // Not joined: a grandchild could hold the pipe open past the kill.
            drop(drain);
            return Err(Stop::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = drain.join().unwrap_or_default();
                if status.success() {
                    progress.advance(base + item_bytes);
                    return Ok(());
                }
                let text = text.trim();
                return Err(Stop::Failed(if text.is_empty() {
                    format!("{} failed", ctl.copy_program)
                } else {
                    text.to_string()
                }));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(drain);
                return Err(Stop::Failed(e.to_string()));
            }
        }
        if Instant::now() >= next_measure {
            let started = Instant::now();
            // Only a successful exit completes the item: while the child lives,
            // even a pre-sized stage stays below its full length.
            progress.advance(base + staged_bytes(stage).min(item_bytes.saturating_sub(1)));
            // A walk over a big tree is throttled so measuring stays cheap.
            next_measure = Instant::now() + (started.elapsed() * 4).max(Duration::from_millis(40));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
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
/// Plans the batch with no live mutation.
fn plan_transfer_items(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
) -> Result<Vec<PlanItem>, String> {
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
                PasteConflict::KeepBoth | PasteConflict::KeepBothPlainNumber => {
                    let plain = on_conflict == PasteConflict::KeepBothPlainNumber;
                    let mut n = 1;
                    let candidate = loop {
                        let cand = if n == 1 {
                            Path::new(dest_dir).join(name)
                        } else {
                            Path::new(dest_dir).join(numbered_name(name, n, plain))
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

    Ok(plan)
}

#[cfg(any(test))]
pub fn transfer_batch(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
    step: &dyn Fn(&str) -> Result<(), String>,
    force_cross_device: bool,
) -> Result<TransferResult, String> {
    let ctl = TransferCtl { cancel: &NEVER_CANCELLED, on_event: &|_| {}, copy_program: "cp" };
    transfer_batch_ctl(srcs, dest_dir, is_copy, on_conflict, step, force_cross_device, &ctl)
        .map_err(|fail| match fail {
            TransferFail::Cancelled => CANCELLED_MSG.to_string(),
            TransferFail::Failed(message) => message,
        })
}

/// `transfer_batch` with a cancel token and progress. Cancel is observed while
/// sources are walked and hashed, while each staged item is copied, and around
/// every phase up to the verified commit point. A cancel takes the same
/// rollback as a failure and reports `Cancelled` only when that rollback was
/// complete.
pub fn transfer_batch_ctl(
    srcs: &[String],
    dest_dir: &str,
    is_copy: bool,
    on_conflict: PasteConflict,
    step: &dyn Fn(&str) -> Result<(), String>,
    force_cross_device: bool,
    ctl: &TransferCtl,
) -> Result<TransferResult, TransferFail> {
    use std::os::unix::fs::MetadataExt;
    let cancel = ctl.cancel;
    let fail_of = |stop: Stop| match stop {
        Stop::Cancelled => TransferFail::Cancelled,
        Stop::Failed(message) => TransferFail::Failed(message),
    };
    (ctl.on_event)(TransferEvent::Preparing);
    if !Path::new(dest_dir).is_dir() {
        return Err(TransferFail::Failed(format!("not a folder: {dest_dir}")));
    }
    let dest_dev = fs::metadata(dest_dir)
        .map_err(|e| TransferFail::Failed(e.to_string()))?
        .dev();
    let mut read_total = 0u64;
    for src in srcs {
        read_total = add_bytes(read_total, count_bytes(Path::new(src), cancel).map_err(|e| fail_of(e.into()))?)
            .map_err(TransferFail::Failed)?;
    }
    let read_done = std::cell::Cell::new(0u64);
    let read_sent = std::cell::Cell::new(0u64);
    let read_tick = |bytes: u64| {
        let done = read_done.get() + bytes;
        read_done.set(done);
        // About every 4 MiB, so a fast drive is not flooded with events.
        if done >= read_sent.get() + 4 * 1024 * 1024 || done == read_total {
            read_sent.set(done);
            (ctl.on_event)(TransferEvent::Reading { done, total: read_total });
        }
    };
    if read_total > 0 {
        (ctl.on_event)(TransferEvent::Reading { done: 0, total: read_total });
    }
    let before = snapshot_sources_tick(srcs, cancel, &read_tick).map_err(|e| fail_of(e.into()))?;
    let mut plan = plan_transfer_items(srcs, dest_dir, is_copy, on_conflict)
        .map_err(TransferFail::Failed)?;

    let cross = |item: &PlanItem| -> Result<bool, String> {
        Ok(force_cross_device
            || fs::symlink_metadata(&item.src).map_err(|e| e.to_string())?.dev() != dest_dev)
    };
    // Bytes to stage for the whole batch, known before any copy starts.
    let mut total = 0u64;
    let mut staging = false;
    let mut sizes: Vec<u64> = Vec::new();
    for item in &plan {
        let mut bytes = 0;
        if is_copy || cross(item).map_err(TransferFail::Failed)? {
            staging = true;
            bytes = count_bytes(Path::new(&item.src), cancel).map_err(|e| fail_of(e.into()))?;
            total = add_bytes(total, bytes).map_err(TransferFail::Failed)?;
        }
        sizes.push(bytes);
    }
    // Bytes hashed to prove the result: the staged copy (when there is one),
    // then the result itself, and for a copy the source again.
    let verify_total = add_bytes(total, read_total.saturating_mul(if is_copy { 2 } else { 1 }))
        .map_err(TransferFail::Failed)?;
    let verify_done = std::cell::Cell::new(0u64);
    let verify_sent = std::cell::Cell::new(0u64);
    let verify_tick = |bytes: u64| {
        let done = (verify_done.get() + bytes).min(verify_total);
        verify_done.set(done);
        if done >= verify_sent.get() + 4 * 1024 * 1024 || done == verify_total {
            verify_sent.set(done);
            (ctl.on_event)(TransferEvent::Checking { done, total: verify_total });
        }
    };
    let mut progress = Progress { total, last: None, on_event: ctl.on_event };
    if staging {
        progress.advance(0);
    }

    let mut stages: Vec<PathBuf> = Vec::new();
    let mut journal: Vec<JournalOp> = Vec::new();
    let mut retire: Vec<PathBuf> = Vec::new();
    // Cancel is checked before and after every phase.
    let phase = |name: &str| -> Result<(), Stop> {
        check_cancel(cancel)?;
        step(name)?;
        check_cancel(cancel)?;
        Ok(())
    };
    // Called once a live change is journalled: the seam for cancelling after a
    // real mutation (`backed-up`, `held`, `placed`), then the cancel check.
    let mutated = |name: &str| -> Result<(), Stop> {
        step(name)?;
        check_cancel(cancel)?;
        Ok(())
    };
    let report_cell: std::cell::RefCell<Vec<ReportRow>> = std::cell::RefCell::new(Vec::new());
    let mut run = || -> Result<(), Stop> {
        // Prepare every incoming object beside its destination.
        let mut base = 0u64;
        for (index, item) in plan.iter_mut().enumerate() {
            check_cancel(cancel)?;
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
                let item_bytes = sizes[index];
                (ctl.on_event)(TransferEvent::Item { index, bytes: item_bytes });
                copy_item(ctl, &item.src, &stage, base, item_bytes, &mut progress)?;
                base += item_bytes;
                (ctl.on_event)(TransferEvent::Verifying);
                if fingerprint_tick(&stage_str, cancel, &verify_tick)? != *src_fp {
                    return Err(Stop::Failed("the prepared copy does not match its source".to_string()));
                }
                item.stage = Some(stage);
            }
            phase("prepare")?;
        }
        // Commit in order, journaling every live change.
        for item in plan.iter() {
            if matches!(item.kind, PlanKind::Replace) {
                phase("backup")?;
                let dir = item.dest.parent().ok_or("bad target")?;
                let backup = internal_sibling(dir, "old");
                fs::rename(&item.dest, &backup).map_err(|e| e.to_string())?;
                journal.push(JournalOp::Backup { target: item.dest.clone(), backup: backup.clone() });
                retire.push(backup);
                mutated("backed-up")?;
            }
            match (&item.stage, is_copy) {
                (Some(stage), true) => {
                    phase("place")?;
                    fs::rename(stage, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Placed { dest: item.dest.clone() });
                    mutated("placed")?;
                }
                (Some(stage), false) => {
                    phase("hold")?;
                    let src = Path::new(&item.src);
                    let hold = internal_sibling(src.parent().ok_or("bad source")?, "hold");
                    fs::rename(src, &hold).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Hold { src: src.to_path_buf(), hold: hold.clone() });
                    retire.push(hold);
                    mutated("held")?;
                    phase("place")?;
                    fs::rename(stage, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Placed { dest: item.dest.clone() });
                    mutated("placed")?;
                }
                (None, _) => {
                    phase("place")?;
                    fs::rename(&item.src, &item.dest).map_err(|e| e.to_string())?;
                    journal.push(JournalOp::Moved {
                        src: PathBuf::from(&item.src),
                        dest: item.dest.clone(),
                    });
                    mutated("placed")?;
                }
            }
        }
        (ctl.on_event)(TransferEvent::Verifying);
        phase("verify")?;
        let items: Vec<ResolvedItem> = plan
            .iter()
            .map(|i| ResolvedItem { from: i.src.clone(), to: i.dest.to_string_lossy().to_string() })
            .collect();
        let rows = verify_transfer_tick(&before, &items, is_copy, cancel, &verify_tick)?;
        *report_cell.borrow_mut() = rows;
        Ok(())
    };
    if let Err(stop) = run() {
        let failures = roll_back(&journal, &stages);
        return Err(match (stop, failures.is_empty()) {
            (Stop::Cancelled, true) => TransferFail::Cancelled,
            (Stop::Cancelled, false) => TransferFail::Failed(format!(
                "cancelled, but rollback incomplete: {}",
                failures.join("; ")
            )),
            (Stop::Failed(err), true) => TransferFail::Failed(err),
            (Stop::Failed(err), false) => TransferFail::Failed(format!(
                "{err}; rollback incomplete: {}",
                failures.join("; ")
            )),
        });
    }
    (ctl.on_event)(TransferEvent::Committing);

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
    let items: Vec<ResolvedItem> = plan
        .iter()
        .map(|i| ResolvedItem { from: i.src.clone(), to: i.dest.to_string_lossy().to_string() })
        .collect();
    let report = report_cell.into_inner();
    if !is_copy {
        notify_path_changes(&items.iter().map(|i| (i.from.clone(), Some(i.to.clone()))).collect::<Vec<_>>());
    }
    Ok(TransferResult { items, warnings, report })
}

/// D-187: moves each item from `item.to` back to `item.from`, refusing the
/// whole batch if any destination (`item.from`) is already occupied, and
/// returns the exact reciprocal (`from`/`to` swapped to where each item now
/// sits) so the opposite stack can move it back again. Self-symmetric: the
/// same function drives both directions of a move, like `reciprocal_rename`.
pub fn restore_moved_items(items: &[ResolvedItem]) -> Result<Vec<ResolvedItem>, String> {
    for item in items {
        let original = Path::new(&item.from);
        if original.exists() {
            return Err(format!("undo target already exists: {}", item.from));
        }
    }
    for item in items {
        run_cmd_capture("mv", &["-T", "--", &item.to, &item.from])?;
        notify_path_changes(&[(item.to.clone(), Some(item.from.clone()))]);
    }
    Ok(items
        .iter()
        .map(|item| ResolvedItem { from: item.to.clone(), to: item.from.clone() })
        .collect())
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

/// Decodes straight into `dest_dir` with whatever the tools do to what is
/// already there. Only for a folder made for this call; Extract Here goes
/// through `extract_archive_here`.
pub fn extract_archive(path: &str, dest_dir: &str) -> Result<(), String> {
    decode_archive(path, &run_cmd_capture, &mut || Ok(dest_dir.to_string())).map(|_| ())
}

/// The one place that knows which tool reads which archive type, and which tool
/// is tried next when the first fails. `out` names the folder each attempt
/// writes into, asked once per attempt; the folder of the attempt that
/// succeeded is returned.
fn decode_archive(
    path: &str,
    run: &dyn Fn(&str, &[&str]) -> Result<(), String>,
    out: &mut dyn FnMut() -> Result<String, String>,
) -> Result<String, String> {
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
        let dir = out()?;
        run("tar", &["-xf", path, "-C", &dir]).map(|()| dir)
    } else if lower.ends_with(".zip") {
        let dir = out()?;
        // D-323: Wayfinder's own reader first (exact progress, instant Cancel,
        // no dependency); it refuses anything it cannot do safely, and 7z does
        // that. Info-ZIP unzip is gone: it failed on a 70 GB zip and 7z is a
        // declared dependency anyway.
        match run(BUILTIN_ZIP, &[path, &dir]) {
            Ok(()) => Ok(dir),
            Err(first) if first == CANCELLED_MSG => Err(first),
            Err(_) => {
                let dir = out()?;
                run("7z", &["x", "-y", &format!("-o{dir}"), path]).map(|()| dir)
            }
        }
    } else if is_archive_path(path) {
        let dir = out()?;
        match run("7z", &["x", "-y", &format!("-o{dir}"), path]) {
            Ok(()) => Ok(dir),
            Err(first) => {
                let dir = out()?;
                run("unar", &["-force-overwrite", "-output-directory", &dir, path])
                    .map(|()| dir)
                    .map_err(|second| format!("{first}; {second}"))
            }
        }
    } else {
        Err("unsupported archive type".to_string())
    }
}

pub fn empty_trash() -> Result<(), String> {
    run_cmd_capture("gio", &["trash", "--empty"])
}

/// The keep-for options the Trash menu offers, newest first. 0 means never.
pub const TRASH_KEEP_CHOICES: [(u32, &str); 5] = [
    (0, "Never delete automatically"),
    (7, "Delete after 7 days"),
    (14, "Delete after 14 days"),
    (30, "Delete after 30 days"),
    (90, "Delete after 90 days"),
];

/// Deletes every Trash item whose recorded deletion date is `keep_days` or
/// more days in the past, and returns how many entries went. `keep_days == 0`
/// means keep forever and touches nothing.
///
/// Both halves of an entry go together: the payload under `files/` and its
/// `.trashinfo`. The rules are deliberately cautious, because nothing is
/// watching this run:
/// * an entry whose date cannot be read is LEFT ALONE, never deleted;
/// * the payload name comes from the info file's own stem, never from the
///   recorded original path, and a stem that is not a plain name is skipped,
///   so nothing outside `files/` can be reached;
/// * a payload that will not delete leaves its `.trashinfo` in place, so the
///   entry still shows in the Trash instead of becoming invisible.
pub fn expire_trash(keep_days: u32) -> Result<usize, String> {
    if keep_days == 0 {
        return Ok(0);
    }
    let now = glib::DateTime::now_local()
        .map_err(|err| err.to_string())?
        .to_unix();
    expire_trash_in(&trash_dir()?, keep_days, now)
}

/// The engine behind `expire_trash`, with the trash directory and "now" both
/// passed in so the rule can be tested against fixed dates on a real trash
/// layout, exactly like `restore_from_trash_in`.
fn expire_trash_in(trash: &Path, keep_days: u32, now: i64) -> Result<usize, String> {
    if keep_days == 0 {
        return Ok(0);
    }
    let info_dir = trash.join("info");
    let files_dir = trash.join("files");
    let entries = match fs::read_dir(&info_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(format!("cannot read {}: {err}", info_dir.display())),
    };
    let cutoff = i64::from(keep_days) * 86_400;
    let mut deleted = 0usize;
    for entry in entries.flatten() {
        let info = entry.path();
        if info.extension().and_then(|ext| ext.to_str()) != Some("trashinfo") {
            continue;
        }
        if !fs::symlink_metadata(&info)
            .map(|meta| meta.file_type().is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(stem) = info.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem.is_empty() || stem.contains('/') || stem == "." || stem == ".." {
            continue;
        }
        let Some((_, date)) = fs::read_to_string(&info)
            .ok()
            .and_then(|content| parse_trashinfo(&content))
        else {
            continue;
        };
        let Some(when) = trashinfo_unix_time(&date) else {
            continue;
        };
        if now.saturating_sub(when) < cutoff {
            continue;
        }
        if remove_any(&files_dir.join(stem)).is_err() {
            continue;
        }
        if fs::remove_file(&info).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// `DeletionDate` as a Unix time. The trash spec writes local time with no
/// zone, so a bare stamp is read as local; one that carries a zone is
/// honoured. Anything unparseable gives `None`, and that entry is kept.
fn trashinfo_unix_time(date: &str) -> Option<i64> {
    let local = glib::TimeZone::local();
    glib::DateTime::from_iso8601(date, Some(&local))
        .ok()
        .map(|parsed| parsed.to_unix())
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

/// D-242, the install-location convention. Nothing on Linux agrees on where
/// hand-installed software goes: packages land in /usr, Flatpaks in
/// ~/.var/app, and an AppImage or a downloaded tarball lands wherever it was
/// dropped. Wayfinder has an opinion, because it is the one program present at
/// both moments that decide it -- the Save dialog and the unpack.
///
/// Package-managed installs are left alone on purpose: dnf, rpm, Flatpak and
/// Snap each have a convention already, and fighting them helps nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// A single self-contained executable file. Belongs in ~/Applications.
    AppImage,
    /// An unpacked program directory. Belongs in ~/opt/<name>.
    AppTree,
}

/// The convention folder for a kind, created on demand by the caller.
pub fn install_dir(kind: InstallKind) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    Ok(install_dir_in(Path::new(&home), kind))
}

/// The same answer for a HOME handed in rather than read from the environment,
/// so a test can prove the rule without touching the real one.
pub fn install_dir_in(home: &Path, kind: InstallKind) -> PathBuf {
    home.join(match kind {
        InstallKind::AppImage => "Applications",
        InstallKind::AppTree => "opt",
    })
}

/// What a NAME alone can settle. An AppImage says what it is in its own
/// extension, so a Save dialog can send it home before it exists. A `.tar.gz`
/// says nothing -- it is as likely to be a backup -- so a tarball is never
/// judged here, only after it is unpacked and its contents can be read.
pub fn install_kind_for_name(name: &str) -> Option<InstallKind> {
    let lower = Path::new(name)
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| name.to_lowercase());
    // A part-written download is still the file it is about to be.
    let lower = lower
        .strip_suffix(".part")
        .or_else(|| lower.strip_suffix(".crdownload"))
        .unwrap_or(&lower)
        .to_string();
    lower.ends_with(".appimage").then_some(InstallKind::AppImage)
}

/// Whether an unpacked folder is a program rather than documents or data: its
/// top level holds a `bin` folder, or a file anyone may execute. Read from the
/// real folder, never guessed from the archive's name.
pub fn looks_like_an_app_tree(dir: &Path) -> bool {
    let kids = |at: &Path| -> Vec<PathBuf> {
        fs::read_dir(at)
            .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default()
    };
    let marked = |list: &[PathBuf]| -> bool {
        list.iter().any(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if path.is_dir() {
                return name == "bin" || name == "usr" || name == "lib";
            }
            is_executable_file(path) || name.ends_with(".AppImage") || name.ends_with(".desktop")
        })
    };
    let top = kids(dir);
    if top.is_empty() {
        return false;
    }
    if marked(&top) {
        return true;
    }
    // A single wrapper folder is the usual tarball shape -- Thing-1.2/bin/... --
    // so look through it once, but only after the top level has had its say.
    match top.as_slice() {
        [only] if only.is_dir() => marked(&kids(only)),
        _ => false,
    }
}

/// An ordinary file carrying any execute bit.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The desktop entry that makes a saved AppImage appear in the menu like any
/// other program. Written beside the user's other entries, never in /usr.
pub fn appimage_desktop_entry(exec: &str, name: &str) -> String {
    // A literal newline or an Exec that splits on spaces would make an entry
    // the desktop silently ignores, so the path is quoted and the name is
    // flattened to one line.
    let name = name.replace(['\n', '\r'], " ");
    let exec = exec.replace('"', "\\\"");
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec=\"{exec}\"\nTerminal=false\nCategories=Utility;\nX-Wayfinder-Installed=true\n"
    )
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

/// One name Extract Here left alone because something already had it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractSkip {
    pub name: String,
    /// The archive's entry is a folder: nothing inside it was extracted either.
    pub folder: bool,
}

/// What one Extract Here did, by top-level name (D-230). Nothing is ever
/// replaced, so each name from the archive is in exactly one of the lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractHereOutcome {
    /// Moved into the destination.
    pub published: Vec<String>,
    /// The name was taken; what was there is as it was.
    pub skipped: Vec<ExtractSkip>,
    /// Could not be moved for any other reason: the name and the system's error.
    pub failed: Vec<(String, String)>,
    /// The temporary folder, when it could not be removed: where it is and why.
    pub leftover: Option<(String, String)>,
}

impl ExtractHereOutcome {
    /// The line the status bar keeps after the folder refreshes. Only a result
    /// with nothing skipped, failed or left behind gets the plain message.
    pub fn summary(&self) -> String {
        if self.skipped.is_empty() && self.failed.is_empty() && self.leftover.is_none() {
            return if self.published.is_empty() {
                "Nothing to extract: the archive is empty".to_string()
            } else {
                "Extracted archive".to_string()
            };
        }
        let mut parts = vec![match self.published.len() {
            0 => "Nothing extracted".to_string(),
            1 => "Extracted 1 item".to_string(),
            n => format!("Extracted {n} items"),
        }];
        if !self.skipped.is_empty() {
            let names: Vec<String> = self
                .skipped
                .iter()
                .map(|skip| if skip.folder { format!("{} (whole folder)", skip.name) } else { skip.name.clone() })
                .collect();
            let exist = if names.len() == 1 { "exists" } else { "exist" };
            parts.push(format!("Skipped {} that already {exist}: {}", names.len(), names.join(", ")));
        }
        if !self.failed.is_empty() {
            let names: Vec<String> = self.failed.iter().map(|(name, why)| format!("{name} ({why})")).collect();
            parts.push(format!("Failed on {}: {}", names.len(), names.join(", ")));
        }
        if let Some((path, why)) = &self.leftover {
            parts.push(format!("Temporary folder not removed: {path} ({why})"));
        }
        parts.join(". ")
    }
}

/// Extract Here (D-230). The archive is decoded into a temporary folder of this
/// call's own inside `dest_dir`, and only a complete decode is moved out, one
/// top-level entry at a time, with a move that cannot replace anything. A name
/// that is taken is skipped whole, folder or not, and reported: what is there
/// is never opened, added to or removed. A failed decode moves nothing.
pub fn extract_archive_here(path: &str, dest_dir: &str) -> Result<ExtractHereOutcome, String> {
    extract_here_with(path, dest_dir, &run_cmd_capture, &rename_no_replace_io)
}

/// `run` starts one tool and `publish` moves one decoded entry into place. They
/// are parameters so a test can fail a tool or take a name at the last moment.
fn extract_here_with(
    path: &str,
    dest_dir: &str,
    run: &dyn Fn(&str, &[&str]) -> Result<(), String>,
    publish: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<ExtractHereOutcome, String> {
    extract_here_dir(path, dest_dir, run, publish, &|_| Ok(()))
}

/// The name `decode_archive` uses for Wayfinder's own zip reader, so it sits in
/// the same try-this-then-that chain as the outside tools.
const BUILTIN_ZIP: &str = "wayfinder-zip";

fn zip_stamp(dt: zip::DateTime) -> Option<std::time::SystemTime> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = dt.year() as i32 - 1900;
    tm.tm_mon = dt.month() as i32 - 1;
    tm.tm_mday = dt.day() as i32;
    tm.tm_hour = dt.hour() as i32;
    tm.tm_min = dt.minute() as i32;
    tm.tm_sec = dt.second() as i32;
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) };
    (secs >= 0).then(|| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

/// Extracts every entry of a zip into the empty folder `dir`. Refuses (so 7z
/// takes over) on anything it will not do safely: an unsafe name, a link, a
/// method the build lacks, a name already written, a bad checksum.
fn extract_zip_builtin(path: &Path, dir: &Path, cancel: &AtomicBool, on_bytes: &dyn Fn(u64)) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    // Open every entry first: a zip this reader cannot fully read is refused
    // before a single byte is written, not after tens of gigabytes.
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.enclosed_name().is_none() {
            return Err("unsafe name in zip".to_string());
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err("zip holds a link".to_string());
        }
    }
    let mut written: u64 = 0;
    let mut buf = vec![0u8; 1 << 20];
    let mut dir_stamps: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for i in 0..archive.len() {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED_MSG.to_string());
        }
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let rel = entry.enclosed_name().ok_or_else(|| "unsafe name in zip".to_string())?;
        let target = dir.join(&rel);
        let mode = entry.unix_mode().unwrap_or(0o644);
        if mode & 0o170000 == 0o120000 {
            return Err("zip holds a link".to_string());
        }
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            if let Some(t) = entry.last_modified().and_then(zip_stamp) {
                dir_stamps.push((target, t));
            }
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| format!("{}: {e}", target.display()))?;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED_MSG.to_string());
            }
            let n = entry.read(&mut buf).map_err(|e| format!("{}: {e}", rel.display()))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", target.display()))?;
            written += n as u64;
            on_bytes(written);
        }
        out.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = out.set_permissions(fs::Permissions::from_mode(mode & 0o777));
        }
        if let Some(t) = entry.last_modified().and_then(zip_stamp) {
            let _ = out.set_modified(t);
        }
    }
    for (d, t) in dir_stamps.into_iter().rev() {
        if let Ok(f) = fs::File::open(&d) {
            let _ = f.set_modified(t);
        }
    }
    Ok(())
}

/// What a running Extract Here reports and obeys (D-321).
pub struct ExtractCtl<'a> {
    pub cancel: &'a AtomicBool,
    /// Bytes written so far into the temporary folder, about twice a second.
    pub on_bytes: &'a (dyn Fn(u64) + Sync),
    /// Total uncompressed bytes when the archive can say (a zip's directory).
    pub on_total: &'a (dyn Fn(u64) + Sync),
}

fn dir_apparent_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if meta.is_dir() {
            total += dir_apparent_bytes(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// Runs one tool, and kills it the moment `cancel` is set. A cancelled run
/// returns `CANCELLED_MSG`, which the caller never retries with another tool.
fn run_cmd_cancellable(cmd: &str, args: &[&str], cancel: &AtomicBool) -> Result<(), String> {
    run_cmd_cancellable_with(cmd, args, cancel, &|_| {})
}

fn run_cmd_cancellable_with(cmd: &str, args: &[&str], cancel: &AtomicBool, on_bytes: &dyn Fn(u64)) -> Result<(), String> {
    if cmd == BUILTIN_ZIP {
        return extract_zip_builtin(Path::new(args[0]), Path::new(args[1]), cancel, on_bytes);
    }
    use std::io::Read as _;
    use std::process::Stdio;
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(s) = stderr.as_mut() {
            let mut bytes = Vec::new();
            let _ = s.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).trim().to_string();
        }
        text
    });
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(CANCELLED_MSG.to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else if text.is_empty() {
        Err(format!("{cmd} failed"))
    } else {
        Err(text)
    }
}

/// Extract Here with a visible progress and a working Cancel (D-321). A
/// cancelled extraction removes its temporary folder and publishes nothing.
pub fn extract_archive_here_ctl(path: &str, dest_dir: &str, ctl: &ExtractCtl) -> Result<ExtractHereOutcome, String> {
    if path.to_lowercase().ends_with(".zip") {
        if let Ok(file) = fs::File::open(path) {
            if let Ok(entries) = index_zip(file, &DEFAULT_ZIP_LIMITS, ctl.cancel) {
                (ctl.on_total)(entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum());
            }
        }
    }
    let current: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::SeqCst) {
                let dir = current.lock().ok().and_then(|d| d.clone());
                if let Some(dir) = dir {
                    (ctl.on_bytes)(dir_apparent_bytes(&dir));
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
        let result = extract_here_dir(
            path,
            dest_dir,
            &|cmd, args| run_cmd_cancellable_with(cmd, args, ctl.cancel, ctl.on_bytes),
            &rename_no_replace_io,
            &|dir| {
                if ctl.cancel.load(Ordering::SeqCst) {
                    return Err(CANCELLED_MSG.to_string());
                }
                if let Ok(mut cur) = current.lock() {
                    *cur = Some(dir.to_path_buf());
                }
                Ok(())
            },
        );
        finished.store(true, Ordering::SeqCst);
        result
    })
}

fn extract_here_dir(
    path: &str,
    dest_dir: &str,
    run: &dyn Fn(&str, &[&str]) -> Result<(), String>,
    publish: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    on_dir: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<ExtractHereOutcome, String> {
    let dest = Path::new(dest_dir);
    let mut stage: Option<ExtractStage> = None;
    let mut attempts = 0u32;
    // Every tool gets a new, empty folder: one that fails part-way cannot leave
    // files in what the next tool's output is moved out of.
    let decoded = decode_archive(path, run, &mut || {
        if stage.is_none() {
            stage = Some(ExtractStage::reserve(dest)?);
        }
        let stage = stage.as_ref().expect("reserved just above");
        attempts += 1;
        let out = stage.path.join(format!("attempt-{attempts}"));
        fs::create_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        on_dir(&out)?;
        Ok(out.to_string_lossy().into_owned())
    });
    let listed = decoded.and_then(|out| {
        use std::os::unix::fs::PermissionsExt;
        let out = PathBuf::from(out);
        // A tar of "." gives its folder's own mode to the folder it writes into.
        // That folder is ours: keep it listable and emptiable whatever it was given.
        let _ = fs::set_permissions(&out, fs::Permissions::from_mode(0o700));
        let mut names = Vec::new();
        for entry in fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))? {
            names.push(entry.map_err(|e| format!("{}: {e}", out.display()))?.file_name());
        }
        names.sort();
        Ok((out, names))
    });
    let (out, names) = match listed {
        Ok(found) => found,
        Err(err) => {
            // Nothing has been moved, so the temporary folder is all there is to undo.
            return Err(match stage.map(ExtractStage::remove) {
                Some(Err((left, why))) => format!("{err}. Temporary folder not removed: {left} ({why})"),
                _ => err,
            });
        }
    };
    let mut outcome = ExtractHereOutcome::default();
    for name in names {
        let (from, to) = (out.join(&name), dest.join(&name));
        let shown = name.to_string_lossy().into_owned();
        match publish_staged_entry(&from, &to, publish) {
            Ok(()) => outcome.published.push(shown),
            // The name is taken, whether it was there all along or arrived a moment ago.
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                let folder = fs::symlink_metadata(&from).is_ok_and(|meta| meta.is_dir());
                outcome.skipped.push(ExtractSkip { name: shown, folder });
            }
            // A real failure is that entry's alone: the rest are still tried.
            Err(e) => outcome.failed.push((shown, e.to_string())),
        }
    }
    if let Some(Err(left)) = stage.map(ExtractStage::remove) {
        outcome.leftover = Some(left);
    }
    Ok(outcome)
}

/// Moves one decoded entry into place through `publish`. A folder can only
/// change parent when its owner may write to it (its ".." entry is rewritten),
/// so a read-only folder from the archive gets owner access for the move and
/// its own mode back afterwards, through an open handle rather than by name.
fn publish_staged_entry(
    from: &Path,
    to: &Path,
    publish: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(from)?;
    let mode = meta.permissions().mode() & 0o7777;
    if !meta.is_dir() || mode & 0o200 != 0 {
        return publish(from, to);
    }
    fs::set_permissions(from, fs::Permissions::from_mode(mode | 0o700))?;
    let handle = match fs::File::open(from) {
        Ok(handle) => handle,
        Err(e) => {
            let _ = fs::set_permissions(from, fs::Permissions::from_mode(mode));
            return Err(e);
        }
    };
    let moved = publish(from, to);
    let restored = handle.set_permissions(fs::Permissions::from_mode(mode));
    moved?;
    restored.map_err(|e| std::io::Error::other(format!("extracted, but its permissions could not be put back: {e}")))
}

static EXTRACT_STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The temporary folder one Extract Here decodes into: made new inside the
/// destination, so every move out of it stays on one filesystem, private to the
/// user, and remembered by identity so cleanup removes this folder and no other.
struct ExtractStage {
    path: PathBuf,
    identity: (u64, u64),
}

impl ExtractStage {
    /// Creating the folder fails when the name is taken by anything at all, a
    /// dangling link included, so nothing is ever adopted: the next name is tried.
    fn reserve(dest: &Path) -> Result<Self, String> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        for _ in 0..10_000 {
            let n = EXTRACT_STAGE_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = dest.join(format!(".wf-extract-{}-{n}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    return match fs::symlink_metadata(&path) {
                        Ok(meta) => Ok(Self { path, identity: (meta.dev(), meta.ino()) }),
                        Err(e) => {
                            let _ = fs::remove_dir(&path);
                            Err(format!("{}: {e}", path.display()))
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot create a temporary folder in {}: {e}", dest.display())),
            }
        }
        Err(format!("cannot create a temporary folder in {}: every name tried is taken", dest.display()))
    }

    /// Removes the folder and whatever is left in it, only while its name still
    /// holds the folder made here. `Err` is where it is and why it is still there.
    fn remove(self) -> Result<(), (String, String)> {
        use std::os::unix::fs::MetadataExt;
        let left = |why: String| (self.path.display().to_string(), why);
        match fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.is_dir() && (meta.dev(), meta.ino()) == self.identity => {
                remove_owned_tree(&self.path).map_err(|e| left(e.to_string()))
            }
            Ok(_) => Err(left("something else now has its name".to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(left("it was moved or removed by something else".to_string()))
            }
            Err(e) => Err(left(e.to_string())),
        }
    }
}

/// Removes a folder this operation made, never following a link. An archive can
/// hold folders their owner may not write to or list; those get owner access
/// first, or what is inside them could not be removed.
fn remove_owned_tree(root: &Path) -> std::io::Result<()> {
    if fs::remove_dir_all(root).is_ok() {
        return Ok(());
    }
    grant_owner_access(root);
    fs::remove_dir_all(root)
}

fn grant_owner_access(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            grant_owner_access(&entry.path());
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

/// D-324: what a file's first bytes say it is, as (label, extensions that
/// name that format). `None` when no known signature matches.
pub fn sniff_real_type(head: &[u8]) -> Option<(&'static str, &'static [&'static str])> {
    let starts = |prefix: &[u8]| head.starts_with(prefix);
    if starts(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 && head[1] & 0x06 != 0) {
        return Some(("MP3 audio", &["mp3", "mp2", "mpga"]));
    }
    if starts(b"RIFF") && head.len() >= 12 {
        return match &head[8..12] {
            b"WAVE" => Some(("WAV audio", &["wav", "wave"])),
            b"AVI " => Some(("AVI video", &["avi"])),
            b"WEBP" => Some(("WebP image", &["webp"])),
            b"RMID" => Some(("MIDI music", &["rmi", "mid", "midi"])),
            _ => None,
        };
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some(("a GIF image", &["gif"]));
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some(("a PNG image", &["png"]));
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some(("a JPEG image", &["jpg", "jpeg", "jpe", "thm"]));
    }
    if starts(b"BM") && head.len() >= 14 {
        return Some(("a BMP image", &["bmp"]));
    }
    if starts(b"%PDF") {
        return Some(("a PDF document", &["pdf"]));
    }
    if starts(b"PK\x03\x04") {
        return Some(("a ZIP archive", &["zip", "docx", "xlsx", "pptx", "odt", "jar", "apk"]));
    }
    if starts(b"OggS") {
        return Some(("Ogg audio", &["ogg", "oga", "ogv", "opus"]));
    }
    if starts(b"fLaC") {
        return Some(("FLAC audio", &["flac"]));
    }
    if starts(b"MThd") {
        return Some(("MIDI music", &["mid", "midi", "kar"]));
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some(("MP4 or QuickTime media", &["mp4", "m4a", "m4v", "mov", "3gp", "heic", "avif"]));
    }
    if starts(&[0x30, 0x26, 0xB2, 0x75]) {
        return Some(("Windows Media (ASF)", &["wma", "wmv", "asf"]));
    }
    if starts(b"MZ") {
        return Some(("a Windows program", &["exe", "dll"]));
    }
    None
}

/// D-324: one line saying why a file cannot be previewed, naming what it
/// really is. `wanted` is the word for what its name promised ("audio",
/// "picture"). Reads at most 16 bytes.
pub fn damaged_file_reason(path: &str, wanted: &str) -> String {
    use std::io::Read;
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let mut head = [0u8; 16];
    let read = match fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => n,
        Err(err) => return format!("Cannot read this file: {err}."),
    };
    if read == 0 {
        return format!("Empty file: 0 bytes, so there is no {wanted} to show.");
    }
    match sniff_real_type(&head[..read]) {
        Some((label, exts)) if !exts.contains(&ext.as_str()) => {
            if ext.is_empty() {
                format!("This file is actually {label}, not {wanted}.")
            } else {
                format!("This file is actually {label}, not a .{ext} {wanted}.")
            }
        }
        Some((label, _)) => format!(
            "Unreadable data: the start of the file says {label}, but it is damaged and cannot be decoded."
        ),
        None => format!("Unreadable data: this file has no known {wanted} format signature, so it is damaged or not {wanted} at all."),
    }
}

/// D-324: a sound file that is plainly not sound (empty, or carrying another
/// kind of data such as a GIF). `None` when the player should try it. Costs
/// one 16-byte read.
pub fn not_audio_reason(path: &str) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let read = fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let plainly_wrong = match sniff_real_type(&head[..read]) {
        _ if read == 0 => true,
        Some((_, exts)) => {
            !exts.contains(&ext.as_str()) && !exts.iter().any(|e| AUDIO_EXT.contains(e))
        }
        None => false,
    };
    plainly_wrong.then(|| damaged_file_reason(path, "audio file"))
}

pub fn is_video_path(path: &str) -> bool {
    has_extension(path, &VIDEO_EXT)
}

/// D-327: how long a MIDI file plays, in seconds, worked out from its own
/// bytes because GStreamer's MIDI decoder reports no length.
///
/// Accepts a plain Standard MIDI File (`MThd`) or one wrapped in RIFF `RMID`
/// (`.rmi`). Honours every tempo change (meta FF 51 03) across all tracks,
/// default 500000 microseconds per quarter note. Format 0 and 1 with a
/// ticks-per-quarter division; SMPTE division is read too. The result is the
/// time of the last event of any track. `None` for anything malformed;
/// never panics. Reads at most 16 MiB.
pub fn midi_duration_secs(path: &str) -> Option<f64> {
    use std::io::Read;
    const CAP: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(CAP)
        .read_to_end(&mut bytes)
        .ok()?;
    midi_duration_from_bytes(&bytes)
}

/// The byte-level half of [`midi_duration_secs`], split out so tests need no files.
pub fn midi_duration_from_bytes(bytes: &[u8]) -> Option<f64> {
    let smf = midi_unwrap_rmid(bytes)?;
    if smf.len() < 14 || &smf[0..4] != b"MThd" {
        return None;
    }
    let header_len = u32::from_be_bytes(smf[4..8].try_into().ok()?) as usize;
    if header_len < 6 || 8 + header_len > smf.len() {
        return None;
    }
    let format = u16::from_be_bytes([smf[8], smf[9]]);
    let track_count = u16::from_be_bytes([smf[10], smf[11]]) as usize;
    let division = u16::from_be_bytes([smf[12], smf[13]]);
    if format > 2 || track_count == 0 {
        return None;
    }
    // Ticks per quarter note, or (SMPTE) a fixed ticks-per-second rate.
    let smpte_ticks_per_sec = if division & 0x8000 != 0 {
        let fps = 256 - u32::from(division >> 8); // negative byte, e.g. -25
        let per_frame = u32::from(division & 0xff);
        let rate = f64::from(fps) * f64::from(per_frame);
        if rate <= 0.0 {
            return None;
        }
        Some(rate)
    } else if division == 0 {
        return None;
    } else {
        None
    };
    let tpq = f64::from(division);

    // (absolute tick, microseconds per quarter) for every tempo change.
    let mut tempos: Vec<(u64, u32)> = Vec::new();
    let mut last_tick: u64 = 0;
    let mut pos = 8 + header_len;
    let mut tracks_read = 0usize;
    while tracks_read < track_count && pos + 8 <= smf.len() {
        let id = &smf[pos..pos + 4];
        let len = u32::from_be_bytes(smf[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        // A last track whose length runs past the end is read as far as it goes.
        let end = start.saturating_add(len).min(smf.len());
        pos = start.saturating_add(len);
        if id != b"MTrk" {
            continue;
        }
        tracks_read += 1;
        let track = &smf[start..end];
        let mut i = 0usize;
        let mut tick: u64 = 0;
        let mut running: Option<u8> = None;
        while i < track.len() {
            let (delta, used) = midi_varlen(&track[i..])?;
            i += used;
            tick = tick.checked_add(u64::from(delta))?;
            let first = *track.get(i)?;
            let status = if first & 0x80 != 0 {
                i += 1;
                first
            } else {
                running?
            };
            match status {
                0xFF => {
                    let kind = *track.get(i)?;
                    i += 1;
                    let (mlen, used) = midi_varlen(track.get(i..)?)?;
                    i += used;
                    let mlen = mlen as usize;
                    let body = track.get(i..i.checked_add(mlen)?)?;
                    if kind == 0x51 && mlen == 3 {
                        let us = (u32::from(body[0]) << 16)
                            | (u32::from(body[1]) << 8)
                            | u32::from(body[2]);
                        if us > 0 {
                            tempos.push((tick, us));
                        }
                    }
                    i += mlen;
                    running = None;
                    if kind == 0x2F {
                        last_tick = last_tick.max(tick);
                        break;
                    }
                }
                0xF0 | 0xF7 => {
                    let (slen, used) = midi_varlen(track.get(i..)?)?;
                    i += used;
                    i = i.checked_add(slen as usize)?;
                    if i > track.len() {
                        return None;
                    }
                    running = None;
                }
                0x80..=0xEF => {
                    running = Some(status);
                    i += if matches!(status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                    if i > track.len() {
                        return None;
                    }
                }
                _ => return None,
            }
            last_tick = last_tick.max(tick);
        }
    }
    if tracks_read == 0 {
        return None;
    }
    if let Some(rate) = smpte_ticks_per_sec {
        return Some(last_tick as f64 / rate);
    }
    tempos.sort_by_key(|t| t.0);
    let mut secs = 0.0f64;
    let mut at = 0u64;
    let mut us = 500_000u32;
    for (tick, new_us) in tempos {
        if tick >= last_tick {
            break;
        }
        secs += (tick - at) as f64 * f64::from(us) / 1_000_000.0 / tpq;
        at = tick;
        us = new_us;
    }
    secs += (last_tick - at) as f64 * f64::from(us) / 1_000_000.0 / tpq;
    Some(secs)
}

/// The Standard MIDI File inside `bytes`: the data itself, or the payload of
/// the `data` chunk of a RIFF `RMID` wrapper.
fn midi_unwrap_rmid(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"RMID" {
        return Some(bytes);
    }
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        let end = start.saturating_add(len).min(bytes.len());
        if id == b"data" {
            return Some(&bytes[start..end]);
        }
        // RIFF chunks are padded to an even length.
        pos = start.saturating_add(len).saturating_add(len & 1);
    }
    None
}

/// A MIDI variable-length number: up to four bytes, seven bits each.
fn midi_varlen(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for (n, byte) in bytes.iter().take(4).enumerate() {
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((value, n + 1));
        }
    }
    None
}

/// MIDI file extensions, case-insensitive.
pub fn is_midi_path(path: &str) -> bool {
    has_extension(path, &["mid", "midi", "kar", "rmi"])
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
    let display_path = entry.source.display_string();
    if entry.is_dir {
        return format!("Folder\n{display_path}");
    }
    let kind = kind_label(entry);
    format!(
        "{kind}\n{display_path}\n{}\nModified: {}",
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

/// The exact local date and time behind the friendly text.
///
/// "Just now" and "27 min ago" cannot tell two files apart, and the relative
/// label is all the column has room for, so the full stamp lives in the row's
/// tooltip (D-303). Seconds are included on purpose: that is the part that
/// settles which of two files in the same minute came last.
pub fn fmt_modified_exact(seconds: i64) -> String {
    if seconds <= 0 {
        return "Modified: unknown".to_string();
    }
    let Ok(dt) = glib::DateTime::from_unix_local(seconds) else {
        return format!("Modified: {seconds}");
    };
    let stamp = dt
        .format("%A, %B %d, %Y at %l:%M:%S %p")
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| seconds.to_string());
    format!("Modified: {stamp}")
}

pub fn validate_name(name: &str) -> Result<(), String> {
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

/// The name for attempt `n` (2, 3...) of a clashing `name`. `plain` false is
/// "stem (n).ext" (Duplicate); true is "stemn.ext" (D-361, no space or
/// brackets). A leading dot is not an extension, so ".bashrc" is all stem.
fn numbered_name(name: &str, n: u32, plain: bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    if plain {
        format!("{stem}{n}{ext}")
    } else {
        format!("{stem} ({n}){ext}")
    }
}

fn unique_target(dest_dir: &str, name: &str) -> PathBuf {
    unique_target_styled(dest_dir, name, false)
}

fn unique_target_styled(dest_dir: &str, name: &str, plain: bool) -> PathBuf {
    let base = Path::new(dest_dir);
    let first = base.join(name);
    if !first.exists() {
        return first;
    }
    let mut n = 2;
    loop {
        let cand = base.join(numbered_name(name, n, plain));
        if !cand.exists() {
            return cand;
        }
        n += 1;
    }
}

/// Like `unique_target`, but a name is "taken" when `symlink_metadata`
/// succeeds rather than when `exists()` does. `exists()` follows the link, so a
/// broken symlink already sitting there reads as absent and `symlink` then
/// fails with EEXIST instead of picking the next number.
fn unique_link_target(dest_dir: &str, name: &str) -> PathBuf {
    let base = Path::new(dest_dir);
    let taken = |path: &Path| fs::symlink_metadata(path).is_ok();
    let first = base.join(name);
    if !taken(&first) {
        return first;
    }
    let mut n = 2;
    loop {
        let cand = base.join(numbered_name(name, n, false));
        if !taken(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// The drop menu's "Link": one symbolic link per source inside `dest_dir`,
/// pointing at the source's own path. Nothing is ever overwritten -- a taken
/// name is numbered "name (2)" the way a paste is numbered.
///
/// All or nothing: if any link cannot be made, every link this call already
/// made is removed before the error is returned, so a half-linked folder is
/// never left behind. Returns the created paths in source order, which is what
/// Undo needs to remove exactly them and nothing else.
pub fn link_into(srcs: &[String], dest_dir: &str) -> Result<Vec<String>, String> {
    if srcs.is_empty() {
        return Err("Nothing to link".to_string());
    }
    if !Path::new(dest_dir).is_dir() {
        return Err(format!("Not a folder: {dest_dir}"));
    }
    let mut created: Vec<String> = Vec::new();
    for src in srcs {
        let src_path = Path::new(src);
        let Some(name) = src_path.file_name().and_then(|n| n.to_str()) else {
            return Err(unwind_links(created, format!("Cannot link {src}")));
        };
        let target = unique_link_target(dest_dir, name);
        if let Err(err) = std::os::unix::fs::symlink(src_path, &target) {
            return Err(unwind_links(created, format!("Could not create link: {err}")));
        }
        created.push(target.to_string_lossy().into_owned());
    }
    Ok(created)
}

/// Remove links already made by a `link_into` call that then failed, and return
/// the message to report. Only symlinks are removed, and only the exact paths
/// this call created, so nothing of the user's can be caught by it.
fn unwind_links(created: Vec<String>, message: String) -> String {
    for path in created {
        if fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            let _ = fs::remove_file(&path);
        }
    }
    message
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
    if cmd == BUILTIN_ZIP {
        return extract_zip_builtin(Path::new(args[0]), Path::new(args[1]), &AtomicBool::new(false), &|_| {});
    }
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
        | "avif" | "heic" | "heif" | "avifs" | "jxl" => "image-x-generic",
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

/// D-295: the sidebar tree's answers (child folders of an expanded branch,
/// and whether a drive has any), remembered so a slow mount is asked once.
/// Bounded, so a window that browses a deep tree all day cannot grow without
/// limit, and every answer can be dropped: when a branch is closed, when its
/// folder changed, or when the hidden-files choice changed what it holds.
pub const TREE_CACHE_CAP: usize = 400;

pub type TreeChild = (String, String, bool);

#[derive(Default, Clone)]
pub struct TreeCache {
    children: std::collections::HashMap<String, Vec<TreeChild>>,
    openable: std::collections::HashMap<String, bool>,
    /// Insertion order of `children` keys, oldest first, for the cap.
    order: std::collections::VecDeque<String>,
}

impl TreeCache {
    pub fn children(&self, path: &str) -> Option<&Vec<TreeChild>> {
        self.children.get(path)
    }

    pub fn openable(&self, mount: &str) -> Option<bool> {
        self.openable.get(mount).copied()
    }

    pub fn insert_children(&mut self, path: String, children: Vec<TreeChild>) {
        if self.children.insert(path.clone(), children).is_none() {
            self.order.push_back(path);
        }
        while self.children.len() > TREE_CACHE_CAP {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.children.remove(&oldest);
                }
                None => break,
            }
        }
    }

    pub fn insert_openable(&mut self, mount: String, openable: bool) {
        // One bool per drive: bounded by the number of mounts, but never
        // let a pathological mount list grow it past the same cap.
        if self.openable.len() >= TREE_CACHE_CAP && !self.openable.contains_key(&mount) {
            self.openable.clear();
        }
        self.openable.insert(mount, openable);
    }

    /// Drop the answer for `path` and for everything below it.
    pub fn forget_under(&mut self, path: &str) {
        let below = format!("{}/", path.trim_end_matches('/'));
        let gone = |key: &String| key == path || key.starts_with(&below);
        self.children.retain(|key, _| !gone(key));
        self.order.retain(|key| !gone(key));
        self.openable.retain(|key, _| !gone(key));
    }

    /// A folder changed: its own answer and its parent's (which carries the
    /// "has subfolders" flag for it) are both stale. Children below stay.
    pub fn forget_folder_and_parent(&mut self, path: &str) {
        let trimmed = path.trim_end_matches('/');
        let mut stale = vec![trimmed.to_string()];
        if let Some(index) = trimmed.rfind('/') {
            stale.push(if index == 0 { "/".to_string() } else { trimmed[..index].to_string() });
        }
        for key in &stale {
            self.children.remove(key);
            self.order.retain(|k| k != key);
            self.openable.remove(key);
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
        self.order.clear();
        self.openable.clear();
    }

    pub fn len(&self) -> usize {
        self.children.len() + self.openable.len()
    }
}

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
    let by_path: std::collections::HashMap<&EntrySource, &Entry> =
        current.iter().map(|entry| (&entry.source, entry)).collect();
    if by_path.len() != current.len() {
        return RefreshKind::Structural;
    }
    for new in scanned {
        let Some(old) = by_path.get(&new.source) else {
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
    let scratch = projects.join("Scratch");
    // scripts/headless-gtk-test.sh owns a per-run root and removes it itself,
    // even after an abort or timeout; honour it only inside the Scratch tree.
    let root = match std::env::var_os("WF_TEST_SCRATCH_ROOT").map(PathBuf::from) {
        Some(p) if p.starts_with(&scratch) && p != scratch => p,
        _ => scratch.join("wf-unit-tests"),
    };
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
            source: EntrySource::real(path),
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
    // ---- D-155: progress and cancellation -------------------------------

    /// Stand-in for `cp -a -T -- SRC STAGE` that writes one 1 MiB chunk per
    /// 150 ms, so a controlled fixture is always still copying when cancelled.
    fn slow_copy_program(root: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("slowcp.sh");
        fs::write(
            &path,
            "#!/bin/sh\nsrc=$4; stage=$5; i=0; : > \"$stage\"\n\
             while [ $i -lt 6 ]; do\n\
             dd if=\"$src\" of=\"$stage\" bs=1048576 skip=$i seek=$i count=1 conv=notrunc 2>/dev/null\n\
             i=$((i+1)); sleep 0.15; done\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().to_string()
    }

    fn big_file_fixture(root: &Path) -> (Vec<String>, String) {
        let (src, dest) = (root.join("src"), root.join("dest"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let bytes: Vec<u8> = (0..6 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        fs::write(src.join("big.bin"), bytes).unwrap();
        (vec![src.join("big.bin").to_string_lossy().to_string()], dest.to_string_lossy().to_string())
    }

    type Events = std::cell::RefCell<Vec<(std::time::Instant, TransferEvent)>>;

    #[allow(clippy::too_many_arguments)]
    fn run_ctl(
        srcs: &[String],
        dest: &str,
        is_copy: bool,
        conflict: PasteConflict,
        step: &dyn Fn(&str) -> Result<(), String>,
        cross: bool,
        cancel: &AtomicBool,
        program: &str,
        events: &Events,
        react: &dyn Fn(TransferEvent),
    ) -> Result<TransferResult, TransferFail> {
        let on_event = |e: TransferEvent| {
            events.borrow_mut().push((std::time::Instant::now(), e));
            react(e);
        };
        let ctl = TransferCtl { cancel, on_event: &on_event, copy_program: program };
        transfer_batch_ctl(srcs, dest, is_copy, conflict, step, cross, &ctl)
    }

    fn bytes_of(events: &Events) -> Vec<(u64, u64)> {
        events
            .borrow()
            .iter()
            .filter_map(|(_, e)| if let TransferEvent::Bytes { done, total } = e { Some((*done, *total)) } else { None })
            .collect()
    }

    #[test]
    fn byte_total_counts_regular_file_bytes_only_d155() {
        use std::os::unix::fs::symlink;
        let root = scratch_case("wf-d155-count");
        let tree = root.join("tree");
        fs::create_dir_all(tree.join("empty")).unwrap();
        fs::create_dir_all(tree.join("d/e")).unwrap();
        fs::write(tree.join("one"), vec![1u8; 10]).unwrap();
        fs::write(tree.join("d/e/two"), vec![2u8; 20]).unwrap();
        let outside = root.join("outside.bin");
        fs::write(&outside, vec![3u8; 5000]).unwrap();
        symlink(&outside, tree.join("link-to-big")).unwrap();
        symlink("missing", tree.join("dangling")).unwrap();
        let never = AtomicBool::new(false);
        assert_eq!(count_bytes(&tree, &never).unwrap(), 30, "symlinks and directories carry no bytes");
        assert_eq!(count_bytes(&tree.join("link-to-big"), &never).unwrap(), 0);
        assert_eq!(count_bytes(&tree.join("one"), &never).unwrap(), 10);
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();
        let events = Events::default();
        run_ctl(&[tree.to_string_lossy().to_string()], dest.to_str().unwrap(), true, PasteConflict::KeepBoth,
            &|_| Ok(()), false, &never, "cp", &events, &|_| {}).unwrap();
        let bytes = bytes_of(&events);
        assert_eq!(bytes.first(), Some(&(0, 30)));
        assert_eq!(bytes.last(), Some(&(30, 30)));
    }

    /// D-313: the read of the sources before a copy reports its progress, so a
    /// slow drive shows a moving bar instead of a silent "Preparing".
    #[test]
    fn reading_the_sources_reports_progress_before_any_copy_d313() {
        let root = scratch_case("wf-d313-reading");
        let (srcs, dest) = big_file_fixture(&root);
        let program = slow_copy_program(&root);
        let events = Events::default();
        let never = AtomicBool::new(false);
        let result = run_ctl(&srcs, &dest, true, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, &program, &events, &|_| {});
        assert!(result.is_ok(), "{result:?}");
        let all: Vec<TransferEvent> = events.borrow().iter().map(|(_, e)| *e).collect();
        let total = 6 * 1024 * 1024;
        let reads: Vec<(u64, u64)> = all
            .iter()
            .filter_map(|e| if let TransferEvent::Reading { done, total } = e { Some((*done, *total)) } else { None })
            .collect();
        assert!(reads.len() >= 3, "start, a middle step and the end: {reads:?}");
        assert_eq!(reads.first(), Some(&(0, total)));
        assert_eq!(reads.last(), Some(&(total, total)));
        assert!(reads.windows(2).all(|w| w[1].0 > w[0].0), "strictly advancing: {reads:?}");
        let last_read = all.iter().rposition(|e| matches!(e, TransferEvent::Reading { .. })).unwrap();
        let first_bytes = all.iter().position(|e| matches!(e, TransferEvent::Bytes { .. })).unwrap();
        assert!(last_read < first_bytes, "all reading happens before the first copied byte");
    }

    /// D-316: proving the copy matches reports progress, so "Verifying" on a
    /// slow drive shows a moving bar and a number instead of standing still.
    #[test]
    fn verifying_reports_progress_d316() {
        let root = scratch_case("wf-d316-verifying");
        let (srcs, dest) = big_file_fixture(&root);
        let program = slow_copy_program(&root);
        let events = Events::default();
        let never = AtomicBool::new(false);
        let result = run_ctl(&srcs, &dest, true, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, &program, &events, &|_| {});
        assert!(result.is_ok(), "{result:?}");
        let all: Vec<TransferEvent> = events.borrow().iter().map(|(_, e)| *e).collect();
        let checks: Vec<(u64, u64)> = all
            .iter()
            .filter_map(|e| if let TransferEvent::Checking { done, total } = e { Some((*done, *total)) } else { None })
            .collect();
        // A 6 MiB copy hashes the staged copy, the result and the source: 18 MiB.
        let total = 18 * 1024 * 1024;
        assert!(checks.len() >= 4, "several steps: {checks:?}");
        assert_eq!(checks.last(), Some(&(total, total)));
        assert!(checks.iter().all(|(d, t)| *t == total && d <= t));
        assert!(checks.windows(2).all(|w| w[1].0 > w[0].0), "strictly advancing: {checks:?}");
        let first_check = all.iter().position(|e| matches!(e, TransferEvent::Checking { .. })).unwrap();
        let last_bytes = all.iter().rposition(|e| matches!(e, TransferEvent::Bytes { .. })).unwrap();
        assert!(first_check > last_bytes - 1, "checking follows copying");
    }

    /// D-320: a finished transfer hands back what the final check verified.
    /// D-321: Extract Here reports its total and its bytes written, and a
    /// Cancel set before it starts stops it and leaves nothing behind.
    #[test]
    fn extract_reports_progress_and_cancels_d321() {
        let root = scratch_case("wf-d321-extract");
        let dest = root.join("out");
        fs::create_dir_all(&dest).unwrap();
        let archive = root.join("pack.zip");
        let big = vec![b'x'; 3 * 1024 * 1024];
        {
            let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            let options: zip::write::FileOptions<'_, '_, ()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("a.bin", options).unwrap();
            std::io::Write::write_all(&mut writer, &big).unwrap();
            writer.start_file("b.txt", options).unwrap();
            std::io::Write::write_all(&mut writer, b"hello").unwrap();
            writer.finish().unwrap();
        }
        let total = std::sync::atomic::AtomicU64::new(0);
        let never = AtomicBool::new(false);
        let ctl = ExtractCtl { cancel: &never, on_bytes: &|_| {}, on_total: &|t| total.store(t, Ordering::SeqCst) };
        let done = extract_archive_here_ctl(archive.to_str().unwrap(), dest.to_str().unwrap(), &ctl).expect("extract");
        assert_eq!(total.load(Ordering::SeqCst), 3 * 1024 * 1024 + 5, "the zip's own total");
        assert_eq!(done.published.len(), 2, "{:?}", done.published);
        assert_eq!(fs::read(dest.join("b.txt")).unwrap(), b"hello");

        let dest2 = root.join("out2");
        fs::create_dir_all(&dest2).unwrap();
        let cancelled = AtomicBool::new(true);
        let ctl = ExtractCtl { cancel: &cancelled, on_bytes: &|_| {}, on_total: &|_| {} };
        let err = extract_archive_here_ctl(archive.to_str().unwrap(), dest2.to_str().unwrap(), &ctl).unwrap_err();
        assert_eq!(err, CANCELLED_MSG);
        assert_eq!(fs::read_dir(&dest2).unwrap().count(), 0, "a cancelled extraction leaves nothing");
    }

    /// D-323: the built-in reader keeps contents and modes, reports exact bytes,
    /// refuses an unsafe name or a link (so 7z takes over), and stops when cancelled.
    #[test]
    fn builtin_zip_reader_d323() {
        use std::io::Write as _;
        let root = scratch_case("wf-d323-zip");
        let mk = |name: &str, file: &str, mode: u32, body: &[u8], method: zip::CompressionMethod| -> PathBuf {
            let at = root.join(name);
            let mut w = zip::ZipWriter::new(fs::File::create(&at).unwrap());
            let o: zip::write::FileOptions<'_, '_, ()> =
                zip::write::FileOptions::default().compression_method(method).unix_permissions(mode);
            w.start_file(file, o).unwrap();
            w.write_all(body).unwrap();
            w.finish().unwrap();
            at
        };
        let archive = mk("a.zip", "Movies/a.txt", 0o640, &vec![b'q'; 2_500_000], zip::CompressionMethod::Deflated);
        let out = root.join("out");
        fs::create_dir(&out).unwrap();
        let seen = std::sync::atomic::AtomicU64::new(0);
        extract_zip_builtin(&archive, &out, &AtomicBool::new(false), &|b| seen.store(b, Ordering::SeqCst)).unwrap();
        assert_eq!(fs::metadata(out.join("Movies/a.txt")).unwrap().len(), 2_500_000);
        assert_eq!(seen.load(Ordering::SeqCst), 2_500_000, "exact bytes written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(out.join("Movies/a.txt")).unwrap().permissions().mode() & 0o777, 0o640);
        }

        let evil = mk("evil.zip", "../escape.txt", 0o644, b"x", zip::CompressionMethod::Stored);
        let out2 = root.join("out2");
        fs::create_dir(&out2).unwrap();
        assert!(extract_zip_builtin(&evil, &out2, &AtomicBool::new(false), &|_| {}).is_err());
        assert!(!root.join("escape.txt").exists());

        let link = root.join("link.zip");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&link).unwrap());
            let o: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
            w.add_symlink("l", "/etc/passwd", o).unwrap();
            w.finish().unwrap();
        }
        let out3 = root.join("out3");
        fs::create_dir(&out3).unwrap();
        assert!(extract_zip_builtin(&link, &out3, &AtomicBool::new(false), &|_| {}).is_err());

        let out4 = root.join("out4");
        fs::create_dir(&out4).unwrap();
        assert_eq!(extract_zip_builtin(&archive, &out4, &AtomicBool::new(true), &|_| {}), Err(CANCELLED_MSG.to_string()));
    }

    /// Manual: `WF_REAL_ZIP=<path> cargo test ... real_zip_d323 -- --ignored`. Opens
    /// every entry (so every method is supported) and reads the 20 smallest in
    /// full (so their checksums are checked).
    #[test]
    #[ignore]
    fn real_zip_d323() {
        let path = std::env::var("WF_REAL_ZIP").expect("WF_REAL_ZIP");
        let mut a = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        let mut sizes: Vec<(u64, usize)> = Vec::new();
        let total = a.len();
        for i in 0..total {
            let e = match a.by_index(i) {
                Ok(e) => e,
                Err(err) => {
                    eprintln!("entry {i} of {total} refused: {err}");
                    return;
                }
            };
            assert!(e.enclosed_name().is_some(), "unsafe name {i}");
            assert!(e.unix_mode().map_or(true, |m| m & 0o170000 != 0o120000), "link {i}");
            sizes.push((e.size(), i));
        }
        sizes.sort();
        for (_, i) in sizes.iter().filter(|(sz, _)| *sz > 0).take(20) {
            let mut e = a.by_index(*i).unwrap();
            std::io::copy(&mut e, &mut std::io::sink()).unwrap();
        }
        eprintln!("entries {} ok", a.len());
    }

    #[test]
    fn camera_thumbnail_files_are_images_d322() {
        assert!(is_image(Path::new("/card/DCIM/100CANON/MVI_3935.THM")));
        assert!(is_image(Path::new("clip.thm")));
        assert!(!is_image(Path::new("clip.MOV")));
    }

    #[test]
    fn transfer_returns_a_verified_report_d320() {
        let root = scratch_case("wf-d320-report");
        let src = root.join("src");
        let dest = root.join("dest");
        fs::create_dir_all(src.join("album/sub")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("album/one.txt"), b"hello").unwrap();
        fs::write(src.join("album/sub/two.txt"), b"world!!").unwrap();
        fs::write(src.join("solo.bin"), vec![7u8; 1000]).unwrap();
        let srcs = vec![src.join("album").to_string_lossy().to_string(), src.join("solo.bin").to_string_lossy().to_string()];
        let result = transfer_batch_ctl(
            &srcs,
            &dest.to_string_lossy(),
            false,
            PasteConflict::KeepBoth,
            &|_| Ok(()),
            false,
            &TransferCtl { cancel: &AtomicBool::new(false), on_event: &|_| {}, copy_program: "cp" },
        )
        .expect("move succeeds");
        assert_eq!(result.report.len(), 2, "one row per item: {:?}", result.report);
        let album = result.report.iter().find(|r| r.kind == "folder").expect("folder row");
        assert_eq!(album.files_total, 2);
        assert_eq!(album.bytes, 12);
        let mut names: Vec<&str> = album.files.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["one.txt", "sub/two.txt"]);
        let solo = result.report.iter().find(|r| r.kind == "file").expect("file row");
        assert_eq!((solo.files_total, solo.bytes), (1, 1000));
        assert_eq!(solo.digest.len(), 16);
        assert!(!src.join("solo.bin").exists(), "a move leaves no source");
    }

    #[test]
    fn progress_is_monotonic_bounded_and_ends_at_total_d155() {
        let root = scratch_case("wf-d155-progress");
        let (srcs, dest) = big_file_fixture(&root);
        let program = slow_copy_program(&root);
        let events = Events::default();
        let never = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let result = run_ctl(&srcs, &dest, true, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, &program, &events, &|_| {});
        assert!(result.is_ok(), "{result:?}");
        let all: Vec<TransferEvent> = events.borrow().iter().map(|(_, e)| *e).collect();
        assert_eq!(all.first(), Some(&TransferEvent::Preparing));
        assert_eq!(all.last(), Some(&TransferEvent::Committing));
        let bytes = bytes_of(&events);
        assert!(bytes.len() >= 4, "several advancing updates, got {bytes:?}");
        let total = 6 * 1024 * 1024;
        assert!(bytes.iter().all(|(d, t)| *t == total && d <= t));
        assert!(bytes.windows(2).all(|w| w[1].0 > w[0].0), "strictly advancing: {bytes:?}");
        assert_eq!(bytes.last(), Some(&(total, total)), "100% only when everything is staged");
        let last_bytes = all.iter().rposition(|e| matches!(e, TransferEvent::Bytes { .. })).unwrap();
        assert!(all[last_bytes + 1..].contains(&TransferEvent::Verifying), "checking is not counted as bytes");
        let first_bytes = events.borrow().iter().find(|(_, e)| matches!(e, TransferEvent::Bytes { done, .. } if *done > 0)).unwrap().0;
        eprintln!("D155 MEASURE first byte event {} ms; updates {}; last {:?}; whole copy {} ms",
            first_bytes.duration_since(started).as_millis(), bytes.len(), bytes.last().unwrap(), started.elapsed().as_millis());
        assert_eq!(fingerprint(&srcs[0]).unwrap(), fingerprint(&format!("{dest}/big.bin")).unwrap());
        assert!(no_owned_names(&root));
    }

    #[test]
    fn cancel_mid_file_stops_the_child_and_restores_everything_d155() {
        for is_copy in [true, false] {
            let root = scratch_case("wf-d155-midfile");
            let (srcs, dest) = big_file_fixture(&root);
            let program = slow_copy_program(&root);
            let before = fingerprint(root.to_str().unwrap()).unwrap();
            let events = Events::default();
            let cancel = AtomicBool::new(false);
            let clicked = std::cell::Cell::new(None);
            let react = |e: TransferEvent| {
                if matches!(e, TransferEvent::Bytes { done, .. } if done > 0) && clicked.get().is_none() {
                    clicked.set(Some(std::time::Instant::now()));
                    cancel.store(true, Ordering::SeqCst);
                }
            };
            let result = run_ctl(&srcs, &dest, is_copy, PasteConflict::KeepBoth, &|_| Ok(()), true, &cancel, &program, &events, &react);
            let done_at = std::time::Instant::now();
            assert_eq!(result.unwrap_err(), TransferFail::Cancelled, "copy={is_copy}");
            let latency = done_at.duration_since(clicked.get().unwrap());
            eprintln!("D155 MEASURE cancel click to terminal (copy={is_copy}) {} ms", latency.as_millis());
            assert!(latency < std::time::Duration::from_millis(250), "{latency:?}");
            std::thread::sleep(std::time::Duration::from_millis(400));
            assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "sources and destination exact");
            assert!(no_owned_names(&root), "no stage left");
            assert!(bytes_of(&events).iter().all(|(d, t)| d < t), "never reached 100 percent");
        }
    }

    #[test]
    fn cancel_during_source_scan_and_verify_hashing_d155() {
        let root = scratch_case("wf-d155-scan");
        let (srcs, dest) = batch_fixture(&root);
        let before = fingerprint(root.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(true);
        let events = Events::default();
        let r = run_ctl(&srcs, &dest, true, PasteConflict::KeepBoth, &|_| Ok(()), false, &cancel, "cp", &events, &|_| {});
        assert_eq!(r.unwrap_err(), TransferFail::Cancelled);
        assert!(bytes_of(&events).is_empty(), "stopped before any total was known");
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
        // Cancel arriving while the staged copy is being checked.
        for is_copy in [true, false] {
            let cancel = AtomicBool::new(false);
            let react = |e: TransferEvent| {
                if e == TransferEvent::Verifying {
                    cancel.store(true, Ordering::SeqCst);
                }
            };
            let r = run_ctl(&srcs, &dest, is_copy, PasteConflict::Replace, &|_| Ok(()), true, &cancel, "cp", &Events::default(), &react);
            assert_eq!(r.unwrap_err(), TransferFail::Cancelled, "copy={is_copy}");
            assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before);
            assert!(no_owned_names(&root));
        }
    }

    #[test]
    fn cancel_at_every_phase_restores_exactly_d155() {
        let cases: [(&str, usize); 6] =
            [("prepare", 1), ("prepare", 2), ("backup", 1), ("hold", 2), ("place", 2), ("verify", 1)];
        for conflict in [PasteConflict::KeepBoth, PasteConflict::Replace] {
            for is_copy in [true, false] {
                for force_cross in [false, true] {
                    for (phase, nth) in cases {
                        if (phase == "backup" && conflict == PasteConflict::KeepBoth)
                            || (phase == "hold" && (is_copy || !force_cross))
                        {
                            continue;
                        }
                        let root = scratch_case("wf-d155-phase");
                        let (srcs, dest) = batch_fixture(&root);
                        let before = fingerprint(root.to_str().unwrap()).unwrap();
                        let cancel = AtomicBool::new(false);
                        let seen = std::cell::Cell::new(0);
                        let step = |name: &str| {
                            if name == phase {
                                seen.set(seen.get() + 1);
                                if seen.get() == nth {
                                    cancel.store(true, Ordering::SeqCst);
                                }
                            }
                            Ok(())
                        };
                        let r = run_ctl(&srcs, &dest, is_copy, conflict, &step, force_cross, &cancel, "cp", &Events::default(), &|_| {});
                        let label = format!("{conflict:?} copy={is_copy} cross={force_cross} {phase}#{nth}");
                        assert_eq!(r.unwrap_err(), TransferFail::Cancelled, "{label}");
                        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "exact: {label}");
                        assert!(no_owned_names(&root), "no residue: {label}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_rollback_that_fails_is_never_called_cancelled_d155() {
        let root = scratch_case("wf-d155-rollback");
        let (srcs, dest) = batch_fixture(&root);
        let cancel = AtomicBool::new(false);
        let dest_path = dest.clone();
        // After the old destination was moved to its backup, destroy the backup
        // so it cannot be restored, then cancel.
        let step = |name: &str| {
            if name == "place" {
                for e in fs::read_dir(&dest_path).unwrap().flatten() {
                    if e.file_name().to_string_lossy().starts_with(".wf-replace-old-") {
                        remove_any(&e.path()).unwrap();
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
            }
            Ok(())
        };
        let r = run_ctl(&srcs, &dest, true, PasteConflict::Replace, &step, false, &cancel, "cp", &Events::default(), &|_| {});
        match r {
            Err(TransferFail::Failed(message)) => assert!(message.contains("rollback incomplete"), "{message}"),
            other => panic!("must be a failure with the residue, got {other:?}"),
        }
    }

    #[test]
    fn cancel_after_the_commit_point_is_ignored_d155() {
        for is_copy in [true, false] {
            let root = scratch_case("wf-d155-late");
            let (srcs, dest) = batch_fixture(&root);
            let cancel = AtomicBool::new(false);
            let react = |e: TransferEvent| {
                if e == TransferEvent::Committing {
                    cancel.store(true, Ordering::SeqCst);
                }
            };
            let r = run_ctl(&srcs, &dest, is_copy, PasteConflict::Replace, &|_| Ok(()), true, &cancel, "cp", &Events::default(), &react);
            assert!(r.is_ok(), "committed work stays committed: {r:?}");
            assert!(no_owned_names(&root));
        }
    }

    #[test]
    fn same_filesystem_move_has_no_byte_progress_but_the_same_terminal_state_d155() {
        let root = scratch_case("wf-d155-rename");
        let (srcs, dest) = batch_fixture(&root);
        let events = Events::default();
        let never = AtomicBool::new(false);
        run_ctl(&srcs, &dest, false, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, "cp", &events, &|_| {}).unwrap();
        assert!(bytes_of(&events).is_empty());
        let cancel = AtomicBool::new(false);
        let root2 = scratch_case("wf-d155-rename2");
        let (srcs2, dest2) = batch_fixture(&root2);
        let before = fingerprint(root2.to_str().unwrap()).unwrap();
        let step = |n: &str| { if n == "place" { cancel.store(true, Ordering::SeqCst); } Ok(()) };
        let r = run_ctl(&srcs2, &dest2, false, PasteConflict::KeepBoth, &step, false, &cancel, "cp", &Events::default(), &|_| {});
        assert_eq!(r.unwrap_err(), TransferFail::Cancelled);
        assert_eq!(fingerprint(root2.to_str().unwrap()).unwrap(), before);
    }

    #[test]
    fn staged_copies_keep_modes_times_and_symlinks_d155() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let root = scratch_case("wf-d155-meta");
        let tree = root.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(tree.join("run.sh"), fs::Permissions::from_mode(0o751)).unwrap();
        symlink("run.sh", tree.join("rel-link")).unwrap();
        symlink("/nowhere/at/all", tree.join("dangling")).unwrap();
        assert!(Command::new("touch").args(["-d", "2001-02-03 04:05:06"]).arg(tree.join("run.sh")).status().unwrap().success());
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();
        let never = AtomicBool::new(false);
        run_ctl(&[tree.to_string_lossy().to_string()], dest.to_str().unwrap(), true, PasteConflict::KeepBoth,
            &|_| Ok(()), false, &never, "cp", &Events::default(), &|_| {}).unwrap();
        let copy = dest.join("tree");
        let (a, b) = (fs::metadata(tree.join("run.sh")).unwrap(), fs::metadata(copy.join("run.sh")).unwrap());
        assert_eq!(b.permissions().mode() & 0o7777, 0o751);
        assert_eq!(a.mtime(), b.mtime());
        assert_eq!(fs::read_link(copy.join("rel-link")).unwrap(), Path::new("run.sh"));
        assert_eq!(fs::read_link(copy.join("dangling")).unwrap(), Path::new("/nowhere/at/all"));
    }

    #[test]
    fn cancel_after_each_real_mutation_restores_the_populated_journal_d155() {
        let cases = ["backed-up", "held", "placed"];
        for conflict in [PasteConflict::KeepBoth, PasteConflict::Replace] {
            for is_copy in [true, false] {
                for force_cross in [false, true] {
                    for name in cases {
                        for nth in [1usize, 3] {
                            let hits = std::cell::Cell::new(0);
                            let root = scratch_case("wf-d155-after");
                            let (srcs, dest) = batch_fixture(&root);
                            let before = fingerprint(root.to_str().unwrap()).unwrap();
                            let cancel = AtomicBool::new(false);
                            let journal_seen = std::cell::Cell::new(false);
                            let step = |n: &str| {
                                if n == name {
                                    hits.set(hits.get() + 1);
                                    if hits.get() == nth {
                                        // The mutation has really happened by now.
                                        let changed = fingerprint(root.to_str().unwrap()).unwrap() != before;
                                        journal_seen.set(changed);
                                        cancel.store(true, Ordering::SeqCst);
                                    }
                                }
                                Ok(())
                            };
                            let r = run_ctl(&srcs, &dest, is_copy, conflict, &step, force_cross, &cancel, "cp", &Events::default(), &|_| {});
                            if hits.get() < nth {
                                assert!(r.is_ok(), "seam not reached, so the run committed");
                                continue;
                            }
                            let label = format!("{conflict:?} copy={is_copy} cross={force_cross} {name}#{nth}");
                            assert!(journal_seen.get(), "live tree already changed: {label}");
                            assert_eq!(r.unwrap_err(), TransferFail::Cancelled, "{label}");
                            assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), before, "exact: {label}");
                            assert!(no_owned_names(&root), "no residue: {label}");
                        }
                    }
                }
            }
        }
    }

    /// A copier that pre-sizes the stage to its full length, then finishes late.
    fn presizing_copy_program(root: &Path) -> (String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("presize.sh");
        let marker = root.join("copier-finished");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nsrc=$4; stage=$5\nsize=$(stat -c %s \"$src\")\ntruncate -s \"$size\" \"$stage\"\nsleep 0.5\n\
                 dd if=\"$src\" of=\"$stage\" bs=1048576 conv=notrunc 2>/dev/null\ntouch '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (path.to_string_lossy().to_string(), marker)
    }

    #[test]
    fn full_progress_waits_for_the_copier_to_exit_d155() {
        let root = scratch_case("wf-d155-presize");
        let (srcs, dest) = big_file_fixture(&root);
        let (program, marker) = presizing_copy_program(&root);
        let events = Events::default();
        let never = AtomicBool::new(false);
        let early = std::cell::Cell::new(0);
        let react = |e: TransferEvent| {
            if matches!(e, TransferEvent::Bytes { done, total } if done == total) && !marker.exists() {
                early.set(early.get() + 1);
            }
        };
        let r = run_ctl(&srcs, &dest, true, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, &program, &events, &react);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(early.get(), 0, "no 100% before the child exited");
        let bytes = bytes_of(&events);
        assert_eq!(bytes.last(), Some(&(6 * 1024 * 1024, 6 * 1024 * 1024)));
        assert!(bytes.iter().filter(|(d, t)| d == t).count() == 1, "one 100% event: {bytes:?}");
        assert_eq!(fingerprint(&srcs[0]).unwrap(), fingerprint(&format!("{dest}/big.bin")).unwrap());
    }

    #[test]
    fn zero_byte_batches_have_a_defined_terminal_state_and_totals_never_wrap_d155() {
        use std::os::unix::fs::symlink;
        let root = scratch_case("wf-d155-zero");
        let (src, dest) = (root.join("src"), root.join("dest"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("empty"), "").unwrap();
        symlink("empty", src.join("link")).unwrap();
        let srcs: Vec<String> = ["empty", "link"].iter().map(|n| src.join(n).to_string_lossy().to_string()).collect();
        let events = Events::default();
        let never = AtomicBool::new(false);
        let r = run_ctl(&srcs, dest.to_str().unwrap(), true, PasteConflict::KeepBoth, &|_| Ok(()), false, &never, "cp", &events, &|_| {});
        assert_eq!(r.unwrap().items.len(), 2);
        assert_eq!(bytes_of(&events), vec![(0, 0)], "one honest 0 of 0 event");
        assert!(dest.join("empty").exists() && fs::read_link(dest.join("link")).is_ok());
        assert_eq!(add_bytes(u64::MAX - 1, 1), Ok(u64::MAX));
        assert!(add_bytes(u64::MAX, 1).unwrap_err().contains("too large"));
    }

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

    /// D-236: the drop menu's Link. Real symlinks, Explorer numbering on a
    /// clash, and nothing of the user's touched.
    #[test]
    fn link_into_creates_symlinks_and_never_overwrites_d236() {
        let dir = test_dir("link-into");
        let src_dir = dir.join("src");
        let dest = dir.join("dest");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src_dir.join("note.txt"), "body").unwrap();
        fs::create_dir(src_dir.join("folder")).unwrap();
        let srcs = vec![
            src_dir.join("note.txt").to_string_lossy().into_owned(),
            src_dir.join("folder").to_string_lossy().into_owned(),
        ];
        let dest_str = dest.to_str().unwrap();

        let made = link_into(&srcs, dest_str).unwrap();
        assert_eq!(
            made,
            vec![
                dest.join("note.txt").to_string_lossy().into_owned(),
                dest.join("folder").to_string_lossy().into_owned(),
            ],
            "returned in source order, so Undo removes exactly these"
        );
        // Real symlinks, pointing at the sources, readable through the link.
        for path in &made {
            assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        }
        assert_eq!(fs::read_link(dest.join("note.txt")).unwrap(), src_dir.join("note.txt"));
        assert_eq!(fs::read_to_string(dest.join("note.txt")).unwrap(), "body");
        assert!(dest.join("folder").is_dir(), "a link to a folder resolves as a folder");

        // A second drop numbers instead of overwriting, and the first link is
        // left exactly as it was.
        let again = link_into(&srcs, dest_str).unwrap();
        assert_eq!(
            again,
            vec![
                dest.join("note (2).txt").to_string_lossy().into_owned(),
                dest.join("folder (2)").to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(fs::read_link(dest.join("note.txt")).unwrap(), src_dir.join("note.txt"));

        // A real file of the user's with the next name is never replaced.
        fs::write(dest.join("note (3).txt"), "keep").unwrap();
        let third = link_into(&srcs[..1], dest_str).unwrap();
        assert_eq!(third, vec![dest.join("note (4).txt").to_string_lossy().into_owned()]);
        assert_eq!(fs::read_to_string(dest.join("note (3).txt")).unwrap(), "keep");

        fs::remove_dir_all(&dir).ok();
    }

    /// A broken link already holding the name counts as taken. `exists()`
    /// follows the link and reports false for one, which is why `link_into`
    /// asks `symlink_metadata` instead: otherwise `symlink` fails with EEXIST
    /// and the whole drop is reported as an error.
    #[test]
    fn link_into_sees_a_broken_link_as_a_taken_name_d236() {
        let dir = test_dir("link-broken");
        let dest = dir.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dir.join("note.txt"), "body").unwrap();
        std::os::unix::fs::symlink(dir.join("gone"), dest.join("note.txt")).unwrap();
        assert!(!dest.join("note.txt").exists(), "a broken link reads as absent");

        let srcs = vec![dir.join("note.txt").to_string_lossy().into_owned()];
        let made = link_into(&srcs, dest.to_str().unwrap()).unwrap();
        assert_eq!(made, vec![dest.join("note (2).txt").to_string_lossy().into_owned()]);
        assert_eq!(fs::read_link(dest.join("note.txt")).unwrap(), dir.join("gone"), "the broken link is untouched");

        fs::remove_dir_all(&dir).ok();
    }

    /// All or nothing: a source that cannot be linked removes the links the
    /// same call already made, so no half-linked folder is left behind.
    #[test]
    fn link_into_unwinds_everything_it_made_when_one_fails_d236() {
        let dir = test_dir("link-unwind");
        let dest = dir.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        // A destination that is not writable makes the second link fail.
        let srcs = vec![
            dir.join("a.txt").to_string_lossy().into_owned(),
            dir.join("b.txt").to_string_lossy().into_owned(),
        ];
        fs::write(dir.join("b.txt"), "b").unwrap();
        // Force the failure by taking the write bit away after the first link.
        let made = link_into(&srcs[..1], dest.to_str().unwrap()).unwrap();
        assert_eq!(made.len(), 1);
        fs::remove_file(&made[0]).unwrap();
        let mut perms = fs::metadata(&dest).unwrap().permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o500);
        }
        fs::set_permissions(&dest, perms).unwrap();
        let err = link_into(&srcs, dest.to_str().unwrap()).unwrap_err();
        assert!(err.starts_with("Could not create link:"), "got {err}");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(
            fs::read_dir(&dest).unwrap().next().is_none(),
            "a failed drop leaves no links behind at all"
        );

        // Empty input and a destination that is not a folder are refused
        // before anything is created.
        assert_eq!(link_into(&[], dest.to_str().unwrap()).unwrap_err(), "Nothing to link");
        assert!(link_into(&srcs, dir.join("a.txt").to_str().unwrap()).unwrap_err().starts_with("Not a folder:"));

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
        let listing = list_dir_cancellable(&dir, false, &cancelled).unwrap();
        let entry = listing
            .entries
            .iter()
            .find(|entry| entry.name == "missing-video.mp4")
            .expect("a metadata failure must not hide the filename");
        assert!(entry.metadata_error.is_some());
        assert_eq!(listing.metadata_unavailable, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    /// D-82: a big folder is statted on several threads. The listing must be
    /// exactly what a serial pass gives: every visible entry once, hidden files
    /// filtered, broken links kept and counted, and a cancel still honoured.
    #[test]
    fn large_listing_split_across_threads_matches_the_folder_d82() {
        let dir = test_dir("large-listing");
        let total = 5000usize;
        for i in 0..total {
            fs::write(dir.join(format!("f{i:05}.txt")), vec![b'x'; i % 7]).unwrap();
        }
        fs::create_dir(dir.join("adir")).unwrap();
        fs::write(dir.join(".hidden"), "h").unwrap();
        #[cfg(unix)]
        for i in 0..3 {
            std::os::unix::fs::symlink(dir.join("nope"), dir.join(format!("broken{i}"))).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let listing = list_dir_cancellable(&dir, false, &cancelled).unwrap();
        assert_eq!(listing.entries.len(), total + 1 + 3, "files, the folder and the broken links; not .hidden");
        assert_eq!(listing.metadata_unavailable, 3);
        assert_eq!(listing.unreadable_entries, 0);
        assert!(listing.entries[0].is_dir && listing.entries[0].name == "adir", "folders first");
        let mut names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), listing.entries.len(), "no entry lost or doubled across chunks");
        let f = listing.entries.iter().find(|e| e.name == "f00006.txt").unwrap();
        assert_eq!((f.size, f.is_dir, f.metadata_error.is_none()), (6, false, true));
        let with_hidden = list_dir_cancellable(&dir, true, &cancelled).unwrap();
        assert_eq!(with_hidden.entries.len(), listing.entries.len() + 1);
        cancelled.store(true, Ordering::Relaxed);
        assert!(matches!(
            list_dir_cancellable(&dir, false, &cancelled),
            Err(ListDirError::Cancelled)
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelled_directory_load_stops_before_opening_the_folder() {
        let dir = test_dir("cancelled-listing");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            list_dir_cancellable(&dir, false, &cancelled),
            Err(ListDirError::Cancelled)
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uri_list_is_crlf_separated_percent_encoded_file_uris() {
        // Browsers require file:// URIs, CRLF separators, and percent-encoded
        // spaces — a bare path or LF separator makes the drop silently fail.
        let paths = vec![
            "/home/user/Downloads/a b.png".to_string(),
            "/home/user/x.jpg".to_string(),
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

    /// D-271: the Trash that receives an item is the one on the item's own
    /// filesystem. Before this, every item was looked for in the home Trash, so
    /// deleting from a mounted drive moved the file and then reported a failure.
    #[test]
    fn trash_dir_follows_the_item_s_own_filesystem_d271() {
        use std::os::unix::fs::MetadataExt;
        let home = std::env::var("HOME").expect("HOME");
        let home_trash = Path::new(&home).join(".local/share/Trash");

        // Something on the home filesystem still uses the home Trash.
        let on_home = format!("{home}/.wf-d271-probe");
        assert_eq!(trash_dir_for(&on_home).unwrap(), home_trash);

        // mount_root_of climbs to the top of the device the path sits on, and
        // never past it.
        let root = mount_root_of(Path::new(&on_home)).expect("a mount root");
        assert!(
            Path::new(&home).starts_with(&root),
            "{root:?} must contain the home folder"
        );
        assert_eq!(
            mount_root_of(Path::new("/proc/self")),
            Some(PathBuf::from("/proc")),
            "/proc is its own filesystem"
        );

        // An item on a different filesystem resolves to that volume's Trash,
        // named for this user, not to the home one.
        let elsewhere = trash_dir_for("/proc/self/cmdline").unwrap();
        assert_ne!(elsewhere, home_trash);
        assert!(
            elsewhere.starts_with("/proc"),
            "must be the volume's own Trash: {elsewhere:?}"
        );
        let uid = fs::metadata(&home).unwrap().uid();
        assert!(
            elsewhere.ends_with(format!(".Trash-{uid}"))
                || elsewhere.ends_with(Path::new(".Trash").join(uid.to_string())),
            "must be this user's Trash on that volume: {elsewhere:?}"
        );
    }

    #[test]
    fn trash_paths_batch_deletes_without_per_file_subprocess() {
        // Reproduces the batch-delete freeze: this used to spawn one `gio trash`
        // process per file and block the UI thread on each.
        //
        // Must live on the real $HOME filesystem: GIO refuses to trash from tmpfs
        // ("system internal mounts"), so /tmp cannot exercise this path.
        //
        // Also serialized against the other real-Trash tests: it doesn't rely on
        // a before/after diff itself, but trashing into the shared real Trash
        // concurrently with a test that does (trash_paths_exact) would corrupt
        // that other test's diff.
        let _guard = REAL_TRASH_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    fn trash_paths_exact_restores_the_right_one_of_two_same_original_path_entries() {
        // D-187: two items trashed separately that shared one original path must
        // each restore to their own exact payload, never "newest by original
        // path" (that heuristic is what `restore_from_trash` uses and is exactly
        // the bug this gate exists to prove fixed for Redo).
        let _guard = REAL_TRASH_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::var("HOME").expect("HOME");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let stem = format!("wf-trashexact-{}-{nanos}", std::process::id());
        let dir = PathBuf::from(&home).join(".cache").join(&stem);
        fs::create_dir_all(&dir).unwrap();
        let original = dir.join("same-name.txt");
        let trash_files = PathBuf::from(&home).join(".local/share/Trash/files");
        let trash_info = PathBuf::from(&home).join(".local/share/Trash/info");
        let mut cleanup: Vec<String> = Vec::new();
        // The receipts of a batch that moved everything; a batch that stopped
        // is this test's error, as it was when the batch returned one.
        let trash_whole = |path: String| -> Result<Vec<TrashReceipt>, String> {
            let batch = trash_paths_exact(&[path]);
            match batch.stopped {
                None => Ok(batch.done.into_iter().map(|item| item.receipt).collect()),
                Some(stop) => Err(format!("{stop:?}")),
            }
        };

        let mut run = || -> Result<(), String> {
            fs::write(&original, b"first").unwrap();
            let first = trash_whole(original.to_string_lossy().to_string())?;
            assert_eq!(first.len(), 1);
            cleanup.push(
                Path::new(&first[0].selector)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            );

            fs::write(&original, b"second").unwrap();
            let second = trash_whole(original.to_string_lossy().to_string())?;
            assert_eq!(second.len(), 1);
            cleanup.push(
                Path::new(&second[0].selector)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            );

            assert_ne!(first[0].selector, second[0].selector, "distinct payloads");

            // Restore the SECOND one by its exact receipt; the first must stay in Trash.
            restore_from_trash_exact(&second)?;
            assert_eq!(
                fs::read(&original).unwrap(),
                b"second",
                "restored the exact payload this receipt names, not the oldest/newest"
            );
            assert!(
                Path::new(&first[0].selector).exists(),
                "the other same-original-path entry must still be in Trash, untouched"
            );

            // Now restore the first — but the original path is occupied again (by
            // the restored "second"); it must refuse without overwriting.
            restore_from_trash_exact(&first).expect_err("destination is occupied; must refuse");
            assert_eq!(
                fs::read(&original).unwrap(),
                b"second",
                "a refused restore must not overwrite"
            );
            Ok(())
        };

        let result = run();
        fs::remove_file(&original).ok();
        for stem in &cleanup {
            fs::remove_file(trash_files.join(stem)).ok();
            fs::remove_file(trash_info.join(format!("{stem}.trashinfo"))).ok();
        }
        fs::remove_dir_all(&dir).ok();
        result.expect("exact trash/restore round trip should succeed");
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

    // D-303: the column is relative text, so two files a few seconds apart read
    // the same. The tooltip is the only place the order is legible.
    #[test]
    fn fmt_modified_exact_carries_the_date_and_the_seconds() {
        let stamp = fmt_modified_exact(1_763_000_000);
        assert!(stamp.starts_with("Modified: "), "{stamp}");
        assert_eq!(stamp.matches(':').count(), 3, "needs h:m:s, got {stamp}");
        assert!(stamp.contains("2025") || stamp.contains("2026"), "{stamp}");
        assert!(!stamp.contains("  "), "padding not collapsed: {stamp}");
    }

    #[test]
    fn fmt_modified_exact_says_unknown_rather_than_a_fake_date() {
        assert_eq!(fmt_modified_exact(0), "Modified: unknown");
        assert_eq!(fmt_modified_exact(-1), "Modified: unknown");
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

    /// The keep-for rule: old entries go whole, young ones stay, and anything
    /// the rule cannot be sure about is kept rather than deleted.
    #[test]
    fn expire_trash_deletes_only_entries_past_the_keep_window() {
        let root = test_dir("trash-expiry");
        let trash = root.join("Trash");
        let origin = root.join("F");
        fs::create_dir_all(&origin).unwrap();
        // 2026-03-01T00:00:00 local is "now" for this test.
        let now = glib::DateTime::from_iso8601("2026-03-01T00:00:00", Some(&glib::TimeZone::local()))
            .unwrap()
            .to_unix();

        // 0 days is off and must not touch a thing, however old the entry.
        trash_entry(&root, "ancient.txt", &origin.join("ancient.txt"), "2019-01-01T00:00:00", b"x");
        assert_eq!(expire_trash_in(&trash, 0, now).unwrap(), 0);
        assert!(trash.join("files/ancient.txt").exists());
        fs::remove_file(trash.join("files/ancient.txt")).unwrap();
        fs::remove_file(trash.join("info/ancient.txt.trashinfo")).unwrap();

        // 40 days old, 20 days old, exactly 30 days old, and an unreadable date.
        trash_entry(&root, "old.txt", &origin.join("old.txt"), "2026-01-20T00:00:00", b"old");
        trash_entry(&root, "young.txt", &origin.join("young.txt"), "2026-02-09T00:00:00", b"young");
        trash_entry(&root, "edge.txt", &origin.join("edge.txt"), "2026-01-30T00:00:00", b"edge");
        trash_entry(&root, "nodate.txt", &origin.join("nodate.txt"), "not a date", b"nodate");
        // A folder goes whole, not file by file.
        trash_entry(&root, "folder", &origin.join("folder"), "2026-01-01T00:00:00", b"");
        let folder = trash.join("files/folder");
        fs::remove_file(&folder).unwrap();
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::write(folder.join("sub/deep.txt"), b"deep").unwrap();

        assert_eq!(expire_trash_in(&trash, 30, now).unwrap(), 3);
        for gone in ["old.txt", "edge.txt", "folder"] {
            assert!(!trash.join("files").join(gone).exists(), "{gone} should be deleted");
            assert!(
                !trash.join("info").join(format!("{gone}.trashinfo")).exists(),
                "{gone}'s record should go with it"
            );
        }
        for kept in ["young.txt", "nodate.txt"] {
            assert!(trash.join("files").join(kept).exists(), "{kept} should be kept");
            assert!(trash.join("info").join(format!("{kept}.trashinfo")).exists());
        }
        // Nothing outside the trash was touched.
        assert!(origin.exists() && fs::read_dir(&origin).unwrap().count() == 0);
        // A second run with nothing left to do reports nothing.
        assert_eq!(expire_trash_in(&trash, 30, now).unwrap(), 0);
        // A missing trash is not an error.
        assert_eq!(expire_trash_in(&root.join("NoTrash"), 30, now).unwrap(), 0);
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

        // D-225: the folder it came from is gone. It is rebuilt and the item
        // comes back, instead of being stranded in the Trash for ever.
        let gone = dest.join("gone-dir/y.txt");
        trash_entry(&root, "y.txt", &gone, "2026-01-01T00:00:00", b"y");
        restore_from_trash_in(&trash, gone.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&gone).unwrap(), b"y");
        assert!(!trash.join("files/y.txt").exists() && !trash.join("info/y.txt.trashinfo").exists());

        // A FILE sitting where the folder has to go is still a refusal: the
        // rebuild never replaces anything.
        let blocked = dest.join("blocker/z.txt");
        fs::write(dest.join("blocker"), b"not a folder").unwrap();
        trash_entry(&root, "z.txt", &blocked, "2026-01-01T00:00:00", b"z");
        let err = restore_from_trash_in(&trash, blocked.to_str().unwrap()).unwrap_err();
        assert!(err.starts_with("destination folder is blocked by a file"), "{err}");
        assert_eq!(fs::read(dest.join("blocker")).unwrap(), b"not a folder");
        assert!(trash.join("files/z.txt").exists() && trash.join("info/z.txt.trashinfo").exists());
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
    /// Searching from `/` walked into /proc, /sys and the Google Drive FUSE
    /// mount and never came back: the status bar said "Searching /" and that
    /// was the whole of it (9 November 2026).
    fn a_search_does_not_walk_into_the_kernel_or_a_network_drive() {
        let mounts = "\
/dev/sda2 / ext4 rw 0 0
proc /proc proc rw 0 0
sysfs /sys sysfs rw 0 0
/dev/sdb1 /run/media/testuser/Orchard ext4 rw 0 0
/dev/sdc1 /run/media/testuser/Lantern fuseblk rw 0 0
drive: /home/user/GoogleDrive fuse.rclone rw 0 0
//nas/share /mnt/nas cifs rw 0 0
";
        let skipped = unsearchable_mounts_from(mounts);
        for gone in ["/proc", "/sys", "/home/user/GoogleDrive", "/mnt/nas"] {
            assert!(skipped.iter().any(|m| m == gone), "{gone} should be skipped");
        }
        // fuseblk is an ordinary NTFS disk, not a FUSE helper.
        for kept in ["/", "/run/media/testuser/Orchard", "/run/media/testuser/Lantern"] {
            assert!(!skipped.iter().any(|m| m == kept), "{kept} is a real disk");
        }
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
        assert_eq!(name_hits[0].source, EntrySource::real(&hit));

        let content_hits = search_dir(dir.to_str().unwrap(), "content only", true, false).unwrap();
        assert_eq!(content_hits.len(), 1);
        assert_eq!(content_hits[0].source, EntrySource::real(&miss));

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
    fn every_sound_format_the_preview_claims_is_recognised() {
        // The preview offers a Play button for exactly this list, so a format
        // dropped from it silently loses the ability to be heard.
        for extension in AUDIO_EXT {
            let lower = format!("/tmp/track.{extension}");
            let upper = format!("/tmp/track.{}", extension.to_uppercase());
            assert!(is_audio_path(&lower), "{lower} was not treated as sound");
            assert!(is_audio_path(&upper), "{upper} was not treated as sound");
        }
    }

    #[test]
    fn no_extension_is_both_sound_and_video() {
        // Both branches return, so an extension in both lists would be
        // decided by whichever check comes first -- a silent coin toss.
        for extension in AUDIO_EXT {
            assert!(
                !VIDEO_EXT.contains(&extension),
                "{extension} is claimed by both the sound and the video list"
            );
        }
    }

    #[test]
    fn playlists_are_text_not_sound() {
        // They list other files rather than containing any, so there is
        // nothing for a Play button to play.
        assert!(!is_audio_path("/tmp/party.m3u"));
        assert!(!is_audio_path("/tmp/party.m3u8"));
        assert!(!is_audio_path("/tmp/album.cue"));
        assert!(!is_audio_path("/tmp/radio.pls"));
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
        let dest_path = PathBuf::from(&dest);
        let path_srcs = vec![PathBuf::from(&a), PathBuf::from(&b)];
        assert!(!drag_is_copy(&path_srcs, &dest_path, false, false), "same device defaults to MOVE");
        assert!(drag_is_copy(&path_srcs, &dest_path, true, false), "Ctrl copies");
        assert!(!drag_is_copy(&path_srcs, &dest_path, false, true), "Shift moves");
        assert!(drag_is_copy(&path_srcs, &dest_path, true, true), "Ctrl wins over Shift");
        // unreadable source or destination: explicit conservative COPY
        assert!(drag_is_copy(&[d.join("missing")], &dest_path, false, false));
        assert!(drag_is_copy(&path_srcs, &d.join("nodir"), false, false));
        // /proc is a different device from a scratch dir
        assert!(drag_is_copy(&[PathBuf::from("/proc/self")], &dest_path, false, false), "different device copies");
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
    fn plain_number_names_d361() {
        let d = scratch_case("wf-addnum");
        let dir = d.to_string_lossy().to_string();
        for name in ["Sheree.jpg", "IMG_1103.JPG", "README", ".bashrc"] {
            fs::write(d.join(name), b"x").unwrap();
        }
        let next = |name: &str| unique_target_styled(&dir, name, true);
        assert_eq!(next("Sheree.jpg"), d.join("Sheree2.jpg"));
        fs::write(d.join("Sheree2.jpg"), b"x").unwrap();
        assert_eq!(next("Sheree.jpg"), d.join("Sheree3.jpg"));
        assert_eq!(next("IMG_1103.JPG"), d.join("IMG_11032.JPG"));
        assert_eq!(next("README"), d.join("README2"));
        assert_eq!(next(".bashrc"), d.join(".bashrc2"));
        assert_eq!(next("free.txt"), d.join("free.txt"));
        // Duplicate's format is unchanged
        assert_eq!(unique_target(&dir, "Sheree.jpg"), d.join("Sheree (2).jpg"));
    }

    #[test]
    fn paste_with_plain_number_keeps_both_d361() {
        let d = scratch_case("wf-addnum-paste");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();
        fs::write(d.join("src/Sheree.jpg"), b"NEW").unwrap();
        fs::write(d.join("dest/Sheree.jpg"), b"OLD").unwrap();
        let src = d.join("src/Sheree.jpg").to_string_lossy().to_string();
        let dest = d.join("dest").to_string_lossy().to_string();
        let mode = conflict_response("add_number").unwrap();
        let out = resolve_paste(&[src.clone()], &dest, true, mode).unwrap();
        assert!(out[0].to.ends_with("Sheree2.jpg"));
        assert_eq!(fs::read(d.join("dest/Sheree.jpg")).unwrap(), b"OLD");
        assert_eq!(fs::read(d.join("dest/Sheree2.jpg")).unwrap(), b"NEW");
        assert_eq!(fs::read(&src).unwrap(), b"NEW");
        // a second paste takes the next free number
        let again = resolve_paste(&[src], &dest, true, mode).unwrap();
        assert!(again[0].to.ends_with("Sheree3.jpg"));
        assert_eq!(fs::read(d.join("dest/Sheree.jpg")).unwrap(), b"OLD");
    }

    #[test]
    fn clash_preflight_and_three_choices() {
        let d = paste_conflict_dir();
        let src = d.join("src/note.txt").to_string_lossy().to_string();
        let dest = d.join("dest").to_string_lossy().to_string();
        fs::write(d.join("src/free.txt"), b"F").unwrap();
        let free = d.join("src/free.txt").to_string_lossy().to_string();
        let dest_path = PathBuf::from(&dest);
        // one clash plus one clean item, for copy and for move
        for is_copy in [true, false] {
            assert_eq!(
                check_conflicts(&[PathBuf::from(&src), PathBuf::from(&free)], &dest_path, is_copy),
                vec!["note.txt"]
            );
        }
        // moving onto its own folder is a no-op, not a clash; copying is a clash
        let own = d.join("dest/note.txt").to_string_lossy().to_string();
        assert!(check_conflicts(&[PathBuf::from(&own)], &dest_path, false).is_empty());
        assert_eq!(check_conflicts(&[PathBuf::from(&own)], &dest_path, true), vec!["note.txt"]);
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
        assert!(is_archive_path("/home/user/Downloads/Tunebox-linux-4.0.0-beta+2 (1).AppImage"));
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
        assert_eq!(archive_stem("/tmp/Tunebox-linux-4.0.0-beta+2 (1).AppImage"), "Tunebox-linux-4.0.0-beta+2 (1)");
    }

    /// D-242: a name settles an AppImage and nothing else. A tarball is as
    /// likely to be a backup as a program, so it is never judged by its name.
    #[test]
    fn install_kind_reads_only_what_a_name_can_prove() {
        use InstallKind::*;
        assert_eq!(install_kind_for_name("/home/user/Downloads/Thing-x86_64.AppImage"), Some(AppImage));
        assert_eq!(install_kind_for_name("thing.appimage"), Some(AppImage), "case does not matter");
        assert_eq!(
            install_kind_for_name("Thing.AppImage.part"),
            Some(AppImage),
            "a part-written download is still what it is about to be"
        );
        assert_eq!(install_kind_for_name("Thing.AppImage.crdownload"), Some(AppImage));
        for name in ["backup.tar.gz", "photos.zip", "notes.txt", "appimage", "Thing.AppImage.txt"] {
            assert_eq!(install_kind_for_name(name), None, "{name} is not settled by its name");
        }
    }

    /// D-242: the two convention folders, under the real HOME, never /usr.
    #[test]
    fn install_dirs_sit_under_home() {
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(install_dir(InstallKind::AppImage).unwrap(), PathBuf::from(&home).join("Applications"));
        assert_eq!(install_dir(InstallKind::AppTree).unwrap(), PathBuf::from(&home).join("opt"));
    }

    /// D-242: a program is told from documents by what is actually in the
    /// folder -- a bin/usr/lib folder, or something executable -- read once
    /// through a single wrapper folder, which is the usual tarball shape.
    #[test]
    fn app_tree_is_told_from_a_folder_of_documents() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch_root().join(format!("wf-d242-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let docs = root.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("a.txt"), "x").unwrap();
        fs::write(docs.join("b.pdf"), "x").unwrap();
        assert!(!looks_like_an_app_tree(&docs), "text and a PDF are not a program");

        let binned = root.join("binned");
        fs::create_dir_all(binned.join("bin")).unwrap();
        assert!(looks_like_an_app_tree(&binned), "a bin folder is a program");

        let wrapped = root.join("wrapped");
        fs::create_dir_all(wrapped.join("Thing-1.2/lib")).unwrap();
        assert!(looks_like_an_app_tree(&wrapped), "one wrapper folder is looked through");

        let exe = root.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let run = exe.join("thing");
        fs::write(&run, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(looks_like_an_app_tree(&exe), "an executable file is a program");
        fs::set_permissions(&run, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!looks_like_an_app_tree(&exe), "the same file without the bit is not");

        assert!(!looks_like_an_app_tree(&root.join("missing")), "a folder that is not there is not a program");
        let _ = fs::remove_dir_all(&root);
    }

    /// D-242: the entry a saved AppImage gets, so it reaches the menu.
    #[test]
    fn appimage_desktop_entry_survives_spaces_and_newlines() {
        let entry = appimage_desktop_entry("/home/user/Applications/My App.AppImage", "My App\nx86_64");
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Exec=\"/home/user/Applications/My App.AppImage\"\n"), "{entry}");
        assert!(entry.contains("Name=My App x86_64\n"), "the name is one line: {entry}");
        assert_eq!(entry.lines().filter(|l| l.starts_with("Name=")).count(), 1);
        assert!(entry.contains("X-Wayfinder-Installed=true"), "ours, and removable as a set");
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
                "delete-speed",
                "details-columns",
                "drive-eject-stick",
                "drive-mounted-again-crucial",
                "drive-open-crucial",
                "drive-open-stick",
                "drive-remount-crucial",
                "drive-remount-stick",
                "drive-space-bars",
                "drive-unmount-crucial",
                "drive-unmount-stick",
                "item-menu-cap-d56",
                "item-menu-full-d60",
                "open-file-other-app",
                "open-with-other-app",
                "rename-in-place",
                "terminal-here-folder",
                "toolbar-redo",
                "toolbar-undo",
                "transfer-cancel",
                "transfer-progress",
                "trash-and-restore-file",
                "zip-archive-row",
                "zip-copy-out",
                "zip-member-open",
            ]
        );
        for (name, entry) in obj {
            let fields = entry.as_object().expect("each entry is an object");
            let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
            names.sort_unstable();
            // D-64: every entry carries `mark`, exactly `circle` or `arrow`; none is optional.
            // D-152: an alias-only entry adds exactly one `alias` field.
            if fields.contains_key("alias") {
                assert_eq!(names, ["alias", "kind", "mark", "where"], "{name}");
                let target = fields["alias"].as_str().expect("alias is a string");
                assert!(!target.is_empty() && target != name, "{name}: alias names a different key");
                let hop = obj.get(target).unwrap_or_else(|| panic!("{name}: alias target {target} exists"));
                assert!(hop.get("alias").is_none(), "{name}: alias is one hop");
            } else {
                assert_eq!(names, ["kind", "mark", "where"], "{name}");
            }
            assert!(matches!(fields["mark"].as_str(), Some("circle" | "arrow")), "{name}: mark is circle or arrow");
            for f in fields.values() {
                assert!(f.as_str().is_some_and(|x| !x.is_empty()), "{name}");
            }
        }
        let mut circles: Vec<&str> =
            obj.iter().filter(|(_, e)| e["mark"].as_str() == Some("circle")).map(|(k, _)| k.as_str()).collect();
        circles.sort_unstable();
        assert_eq!(
            circles,
            [
                "delete-speed",
                "open-file-other-app",
                "rename-in-place",
                "terminal-here-folder",
                "toolbar-redo",
                "toolbar-undo",
                "transfer-cancel",
                "trash-and-restore-file",
                "zip-archive-row",
            ],
            "the toolbar entries and the fixture file item choose circle"
        );
        let mut arrows: Vec<&str> =
            obj.iter().filter(|(_, e)| e["mark"].as_str() == Some("arrow")).map(|(k, _)| k.as_str()).collect();
        arrows.sort_unstable();
        assert_eq!(
            arrows,
            [
                "details-columns",
                "drive-eject-stick",
                "drive-mounted-again-crucial",
                "drive-open-crucial",
                "drive-open-stick",
                "drive-remount-crucial",
                "drive-remount-stick",
                "drive-space-bars",
                "drive-unmount-crucial",
                "drive-unmount-stick",
                "item-menu-cap-d56",
                "item-menu-full-d60",
                "open-with-other-app",
                "transfer-progress",
                "zip-copy-out",
                "zip-member-open",
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

    // ---- D-156: already-mounted network shares ----

    const D156_MOUNTS: &str = "\
/dev/nvme0n1p2 / ext4 rw 0 0
//nas/Media /mnt/nas-media cifs rw 0 0
//bob:hunter2@nas/Media /mnt/nas-media cifs rw 0 0
srv:/export/Photos /run/media/testuser/My\\040Share nfs4 rw 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
gvfsd-fuse /run/user/1001/gvfs fuse.gvfsd-fuse rw 0 0
portal /run/user/1000/doc fuse.portal rw 0 0
appimage /tmp/.mount_App fuse.appimagefs rw 0 0
encfs /home/user/private fuse.encfs rw 0 0
ratarmount /home/user/arch fuse.ratarmount rw 0 0
/dev/loop0 /mnt/local ext4 rw 0 0
tmpfs /run/user/1000 tmpfs rw 0 0
rclone: /home/user/GoogleDrive fuse.rclone rw 0 0
";

    fn d156_children(root: &str) -> Option<Vec<String>> {
        match root {
            "/run/user/1000/gvfs" => Some(vec![
                "smb-share:server=nas,share=Backup".to_string(),
                "sftp:host=example.com,user=bob".to_string(),
                "weird:user=bob,password=hunter2".to_string(),
            ]),
            _ => None, // the 1001 root is unreadable
        }
    }

    /// The rule before D-156: direct nfs/cifs and any fuse.* only under /home.
    fn d156_legacy_rows(mounts: &str) -> Vec<String> {
        mounts
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let (mp, fs) = (f.get(1)?, f.get(2)?);
                ((fs.starts_with("nfs") || *fs == "cifs" || fs.starts_with("fuse."))
                    && mp.starts_with("/home")
                    && !mp.contains("/doc")
                    && !mp.contains("/GoogleDrive")
                    && !fs.contains("rclone"))
                .then(|| mp.to_string())
            })
            .collect()
    }

    #[test]
    fn mount_escapes_decode_d156() {
        assert_eq!(decode_mount_field("/run/media/a\\040b"), "/run/media/a b");
        assert_eq!(decode_mount_field("a\\011b\\134c"), "a\tb\\c");
        assert_eq!(decode_mount_field("plain\\9"), "plain\\9");
        assert_eq!(decode_mount_field("end\\04"), "end\\04");
    }

    #[test]
    fn network_rows_direct_gvfs_dedup_order_and_decoys_d156() {
        let taken: std::collections::HashSet<String> = ["/home/user/GoogleDrive".to_string()].into();
        let rows = network_drives(D156_MOUNTS, &taken, &d156_children);
        let got: Vec<(&str, &str)> = rows.iter().map(|d| (d.name.as_str(), d.path.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("Backup on nas", "/run/user/1000/gvfs/smb-share:server=nas,share=Backup"),
                ("example.com", "/run/user/1000/gvfs/sftp:host=example.com,user=bob"),
                ("Media on nas", "/mnt/nas-media"),
                ("Network share", "/run/user/1000/gvfs/weird:user=bob,password=hunter2"),
                ("Photos on srv", "/run/media/testuser/My Share"),
            ]
        );
        assert!(rows.iter().all(|d| d.kind == "network" && d.mountpoint == d.path && !d.removable));
        for d in &rows {
            for secret in ["hunter2", "bob", "password"] {
                assert!(!d.name.contains(secret), "credential in label: {}", d.name);
            }
        }
        assert!(rows.iter().all(|d| d.path != "/run/user/1000/gvfs"), "aggregate root is not a row");
        assert_eq!(network_drives(D156_MOUNTS, &taken, &d156_children).len(), rows.len(), "stable");
        // a path already found elsewhere is not repeated
        let taken: std::collections::HashSet<String> = ["/mnt/nas-media".to_string()].into();
        assert_eq!(network_drives(D156_MOUNTS, &taken, &d156_children).len(), 4);
        // every root unreadable: direct rows survive
        assert_eq!(network_drives(D156_MOUNTS, &Default::default(), &|_| None).len(), 2);
    }

    #[test]
    fn network_discovery_before_after_and_scale_d156() {
        let before = d156_legacy_rows(D156_MOUNTS);
        let after = network_drives(D156_MOUNTS, &["/home/user/GoogleDrive".to_string()].into(), &d156_children);
        let mut big = String::new();
        for i in 0..1000 {
            big.push_str(&format!("//h{i}/s{i} /mnt/n{i} cifs rw 0 0\n"));
        }
        big.push_str("gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\n");
        let kids: Vec<String> = (0..100).map(|i| format!("smb-share:server=h{i},share=s{i}")).collect();
        let started = std::time::Instant::now();
        let listed = std::cell::Cell::new(0);
        let rows = network_drives(&big, &Default::default(), &|_| {
            listed.set(listed.get() + 1);
            Some(kids.clone())
        });
        let elapsed = started.elapsed();
        assert_eq!(rows.len(), 1100);
        assert_eq!(listed.get(), 1, "one immediate-child listing only");
        println!(
            "D156 MEASURE before: {} rows {:?} (decoy encfs/ratarmount counted, shares missed); after: {} rows; scan of 1000 lines + 100 children: {} rows in {} us",
            before.len(),
            before,
            after.len(),
            rows.len(),
            elapsed.as_micros()
        );
        assert!(elapsed < std::time::Duration::from_millis(500), "{elapsed:?}");
        assert_eq!(after.len(), 5);
    }

    #[test]
    fn rclone_google_drive_is_unchanged_and_not_a_network_row_d156() {
        let rows = network_drives(D156_MOUNTS, &Default::default(), &d156_children);
        // rclone is reached by the existing branch; as a fuse.rclone type it is not remote here
        assert!(rows.iter().all(|d| !d.path.contains("GoogleDrive")));
    }

    #[test]
    fn nfsd_and_local_fuse_are_not_remote_kinds_d156() {
        for yes in ["nfs", "nfs4", "cifs", "smb3", "fuse.sshfs"] {
            assert!(is_remote_fstype(yes), "{yes}");
        }
        for no in ["nfsd", "rpc_pipefs", "fuse.portal", "fuse.appimagefs", "fuse.encfs", "fuse.ratarmount", "fuse", "ext4", "tmpfs", "fuse.rclone"] {
            assert!(!is_remote_fstype(no), "{no}");
        }
        let rows = network_drives("nfsd /proc/fs/nfsd nfsd rw 0 0\n", &Default::default(), &|_| None);
        assert!(rows.is_empty());
    }

    #[test]
    fn network_paths_are_never_probed_but_local_drives_still_are_d156() {
        let mk = |kind: &str, path: &str| Drive {
            name: "x".into(),
            path: path.into(),
            mountpoint: path.into(),
            size: 0,
            fstype: "x".into(),
            kind: kind.into(),
            removable: false,
            space: None,
        };
        let mut drives = vec![
            mk("drive", "/mnt/usb"),
            mk("gdrive", "/home/user/GoogleDrive"),
            mk("network", "/mnt/nas"),
            mk("network", "/run/user/1000/gvfs/smb-share:server=a,share=b"),
        ];
        let probed = std::cell::RefCell::new(Vec::new());
        probe_local_space(&mut drives, &|m| {
            probed.borrow_mut().push(m.to_string());
            Some(Space { total: 10, available: 5 })
        });
        assert_eq!(*probed.borrow(), vec!["/mnt/usb", "/home/user/GoogleDrive"]);
        assert!(drives[2].space.is_none() && drives[3].space.is_none());
        assert!(drives[0].space.is_some() && drives[1].space.is_some());
    }

    #[test]
    fn every_immediate_gvfs_child_is_listed_without_a_cap_d156() {
        let root = scratch_case("wf-d156-many");
        for i in 0..1205 {
            fs::create_dir(root.join(format!("smb-share:server=h,share=s{i}"))).unwrap();
        }
        assert_eq!(gvfs_children(root.to_str().unwrap()).unwrap().len(), 1205);
    }

    #[test]
    fn a_real_directory_root_lists_only_immediate_directories_d156() {
        let root = scratch_case("wf-d156-root");
        fs::create_dir_all(root.join("smb-share:server=a,share=b/deep/deeper")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("file"), "x").unwrap();
        let mut names = gvfs_children(root.to_str().unwrap()).unwrap();
        names.sort();
        assert_eq!(names, vec!["smb-share:server=a,share=b".to_string()]);
        assert!(gvfs_children(root.join("gone").to_str().unwrap()).is_none());
    }

    #[test]
    fn drive_scan_probes_only_mounted_drives_with_the_injected_probe_d46() {
        let probed = std::cell::RefCell::new(Vec::new());
        let drives = list_drives_with(&|mountpoint| {
            probed.borrow_mut().push(mountpoint.to_string());
            Some(Space { total: 1000, available: 1 })
        });
        // Network rows are never probed (a dead share can block), so they stay None.
        let mounted: Vec<_> = drives.iter().filter(|d| !d.mountpoint.is_empty() && d.kind != "network").collect();
        assert_eq!(probed.borrow().len(), mounted.len(), "one probe per mounted non-network drive");
        assert!(drives.iter().filter(|d| d.kind == "network").all(|d| !probed.borrow().contains(&d.mountpoint)));
        for d in &drives {
            let expected = (!d.mountpoint.is_empty() && d.kind != "network").then_some(Space { total: 1000, available: 1 });
            assert_eq!(d.space, expected, "{}", d.name);
        }
        let failed = list_drives_with(&|_| None);
        assert!(failed.iter().all(|d| d.space.is_none()), "a failed probe leaves no space");
    }
}

#[cfg(test)]
mod gvfs_smb_identity_tests_d156_connect {
    use super::*;

    #[test]
    fn gvfs_child_identity_is_decoded_credential_free_and_exact_d156_connect() {
        assert_eq!(gvfs_smb_identity("smb-share:server=nas,share=media"), Some(("nas".into(), "media".into())));
        assert_eq!(
            gvfs_smb_identity("smb-share:domain=W,server=NAS,share=My%20Share,user=bob"),
            Some(("NAS".into(), "My Share".into()))
        );
        assert_eq!(gvfs_smb_identity("sftp:host=nas,share=media"), None);
        assert_eq!(gvfs_smb_identity("smb-share:server=nas"), None);
        assert_eq!(gvfs_smb_identity("smb-share:server=nas,share=bad%zz"), None);
    }

    #[test]
    fn gvfs_smb_shares_reads_only_gvfs_roots_d156_connect() {
        let mounts = "gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\nnas:/x /mnt/x nfs rw 0 0\n";
        let children = |root: &str| {
            assert_eq!(root, "/run/user/1000/gvfs", "only the gvfs root is listed");
            Some(vec!["smb-share:server=nas,share=media".to_string(), "sftp:host=h".to_string()])
        };
        assert_eq!(
            gvfs_smb_shares(mounts, &children),
            vec![("nas".to_string(), "media".to_string(), "/run/user/1000/gvfs/smb-share:server=nas,share=media".to_string())]
        );
    }
}

#[cfg(test)]
mod zip_member_name_safety_tests_d188 {
    use super::*;

    fn empty() -> (HashSet<String>, HashSet<String>) {
        (HashSet::new(), HashSet::new())
    }

    #[test]
    fn rejects_absolute_paths_d188() {
        let (seen, seen_dir) = empty();
        assert_eq!(validate_zip_member_name("/etc/passwd", &seen, &seen_dir, false), Err(ZipNameError::Absolute));
        assert_eq!(validate_zip_member_name("C:/x", &seen, &seen_dir, false), Err(ZipNameError::Absolute));
    }

    #[test]
    fn rejects_parent_traversal_d188() {
        let (seen, seen_dir) = empty();
        assert_eq!(validate_zip_member_name("../x", &seen, &seen_dir, false), Err(ZipNameError::ParentTraversal));
        assert_eq!(validate_zip_member_name("a/../../b", &seen, &seen_dir, false), Err(ZipNameError::ParentTraversal));
    }

    #[test]
    fn rejects_nul_and_backslash_d188() {
        let (seen, seen_dir) = empty();
        assert_eq!(validate_zip_member_name("a\0b", &seen, &seen_dir, false), Err(ZipNameError::NulByte));
        assert_eq!(validate_zip_member_name("a\\b", &seen, &seen_dir, false), Err(ZipNameError::BackslashSeparator));
    }

    #[test]
    fn rejects_ambiguous_normalization_d188() {
        let (seen, seen_dir) = empty();
        assert_eq!(validate_zip_member_name("a//b", &seen, &seen_dir, false), Err(ZipNameError::AmbiguousNormalization));
        assert_eq!(validate_zip_member_name("./a", &seen, &seen_dir, false), Err(ZipNameError::AmbiguousNormalization));
    }

    #[test]
    fn accepts_and_normalizes_a_clean_nested_path_d188() {
        let (seen, seen_dir) = empty();
        assert_eq!(validate_zip_member_name("a/b/c.txt", &seen, &seen_dir, false), Ok("a/b/c.txt".to_string()));
        // A trailing slash marks an explicit directory entry; it normalizes the same way.
        assert_eq!(validate_zip_member_name("a/b/", &seen, &seen_dir, true), Ok("a/b".to_string()));
    }

    #[test]
    fn rejects_duplicate_normalized_paths_d188() {
        let mut seen = HashSet::new();
        seen.insert("a/b.txt".to_string());
        let seen_dir = HashSet::new();
        assert_eq!(validate_zip_member_name("a/b.txt", &seen, &seen_dir, false), Err(ZipNameError::DuplicatePath));
    }

    #[test]
    fn rejects_file_directory_collision_d188() {
        let seen = HashSet::new();
        let mut seen_dir = HashSet::new();
        seen_dir.insert("a/b".to_string());
        assert_eq!(validate_zip_member_name("a/b", &seen, &seen_dir, false), Err(ZipNameError::FileDirectoryCollision));
    }
}

#[cfg(test)]
mod location_tests_d188 {
    use super::*;

    #[test]
    fn zip_read_only_message_is_exact_d188() {
        assert_eq!(ZIP_READ_ONLY_MESSAGE, "ZIP folders are read-only");
    }

    #[test]
    fn zip_root_up_returns_to_the_real_containing_folder_d188() {
        let root = Location::zip_root("/home/user/stuff/a.zip");
        assert_eq!(root.parent(), Location::RealDir(PathBuf::from("/home/user/stuff")));
    }

    #[test]
    fn nested_zip_dir_up_pops_one_internal_segment_d188() {
        let deep = Location::ZipDir { archive: PathBuf::from("/a.zip"), internal: "x/y".to_string() };
        assert_eq!(deep.parent(), Location::ZipDir { archive: PathBuf::from("/a.zip"), internal: "x".to_string() });
    }

    #[test]
    fn child_dir_descends_one_level_d188() {
        let root = Location::zip_root("/a.zip");
        let child = root.child_dir("docs");
        assert_eq!(child, Location::ZipDir { archive: PathBuf::from("/a.zip"), internal: "docs".to_string() });
    }

    #[test]
    fn breadcrumb_tail_names_the_archive_then_each_internal_folder_d188() {
        let loc = Location::ZipDir { archive: PathBuf::from("/home/user/a.zip"), internal: "x/y".to_string() };
        let tail = loc.zip_breadcrumb_tail().unwrap();
        let names: Vec<_> = tail.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(names, vec!["a.zip".to_string(), "x".to_string(), "y".to_string()]);
        assert_eq!(tail[0].1, Location::zip_root("/home/user/a.zip"));
        assert_eq!(tail[2].1, loc);
    }

    #[test]
    fn real_location_has_no_zip_breadcrumb_tail_d188() {
        assert_eq!(Location::real("/home").zip_breadcrumb_tail(), None);
    }
}

#[cfg(test)]
mod zip_index_tests_d188 {
    use super::*;
    use std::io::Cursor;

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buf);
        let options: zip::write::FileOptions<'_, '_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                std::io::Write::write_all(&mut writer, content).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    fn idx(entries: &[(&str, &[u8])]) -> Vec<ZipIndexEntry> {
        let bytes = build_zip(entries);
        index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn indexes_explicit_and_implicit_directories_d188() {
        let entries = idx(&[("a/b/c.txt", b"hi"), ("a/d.txt", b"bye")]);
        let paths: HashSet<_> = entries.iter().map(|e| (e.logical_path.clone(), e.is_dir)).collect();
        assert!(paths.contains(&("a".to_string(), true)), "implicit dir a/");
        assert!(paths.contains(&("a/b".to_string(), true)), "implicit dir a/b/");
        assert!(paths.contains(&("a/b/c.txt".to_string(), false)));
        assert!(paths.contains(&("a/d.txt".to_string(), false)));
    }

    #[test]
    fn zip_root_children_are_folders_first_then_alphabetical_d188() {
        let entries = idx(&[("zeta.txt", b"z"), ("alpha/", b""), ("mid.txt", b"m")]);
        let children = zip_dir_children(Path::new("sample.zip"), 0, &entries, "");
        let names: Vec<_> = children.iter().map(|e| (e.name.clone(), e.is_dir)).collect();
        assert_eq!(names, vec![
            ("alpha".to_string(), true),
            ("mid.txt".to_string(), false),
            ("zeta.txt".to_string(), false),
        ]);
    }

    #[test]
    fn nested_children_only_show_one_level_d188() {
        let entries = idx(&[("a/b/c.txt", b"x"), ("a/d.txt", b"y")]);
        let root_children = zip_dir_children(Path::new("sample.zip"), 0, &entries, "");
        assert_eq!(root_children.len(), 1, "only 'a' at the root");
        let a_children = zip_dir_children(Path::new("sample.zip"), 0, &entries, "a");
        let names: HashSet<_> = a_children.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, HashSet::from(["b".to_string(), "d.txt".to_string()]));
    }

    #[test]
    fn rejects_traversal_name_inside_the_archive_d188() {
        let bytes = build_zip(&[("../escape.txt", b"x")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::ParentTraversal))));
    }

    #[test]
    fn rejects_entry_over_the_claimed_size_limit_d188() {
        let tight = ZipLimits { max_entry_claimed_bytes: 1, ..DEFAULT_ZIP_LIMITS };
        let bytes = build_zip(&[("big.txt", b"way more than one byte")]);
        let result = index_zip(Cursor::new(bytes), &tight, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::EntryTooLarge(_))));
    }

    #[test]
    fn rejects_too_many_members_d188() {
        let tight = ZipLimits { max_members: 1, ..DEFAULT_ZIP_LIMITS };
        let bytes = build_zip(&[("a.txt", b"a"), ("b.txt", b"b")]);
        let result = index_zip(Cursor::new(bytes), &tight, &AtomicBool::new(false));
        assert_eq!(result, Err(ZipIndexError::TooManyMembers));
    }

    #[test]
    fn rejects_implicit_file_directory_collision_file_first_d188() {
        // "a" is a file; "a/b" later implies "a" must also be a directory.
        let bytes = build_zip(&[("a", b"x"), ("a/b", b"y")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::FileDirectoryCollision))), "{result:?}");
    }

    #[test]
    fn rejects_implicit_file_directory_collision_dir_first_d188() {
        // "a/b" is seen first, reserving "a" as a directory; "a" as a file
        // later must still be rejected (order-independent, REJECT 6).
        let bytes = build_zip(&[("a/b", b"y"), ("a", b"x")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::FileDirectoryCollision))), "{result:?}");
    }


    #[test]
    fn rejects_explicit_file_directory_collision_file_first_d188() {
        // Two explicit top-level entries at the same normalized path, one a
        // file and one a directory — REJECT 7's forged collision case, file
        // read first.
        let bytes = build_zip(&[("a", b"x"), ("a/", b"")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::FileDirectoryCollision))), "{result:?}");
    }

    #[test]
    fn rejects_explicit_file_directory_collision_dir_first_d188() {
        // Same forged pair as above, directory read first — the message must
        // be identical regardless of read order (REJECT 7).
        let bytes = build_zip(&[("a/", b""), ("a", b"x")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::FileDirectoryCollision))), "{result:?}");
    }

    #[test]
    fn rejects_duplicate_explicit_directory_first_order_d188() {
        // Two explicit directory entries that normalize to the same path via
        // different raw spellings ("a/" and "a//", both read as directories
        // by the archive format) — a forged true duplicate, not a collision,
        // regardless of which raw form is read first (REJECT 7).
        let bytes = build_zip(&[("a/", b""), ("a//", b"")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::DuplicatePath))), "{result:?}");
    }

    #[test]
    fn rejects_duplicate_explicit_directory_second_order_d188() {
        let bytes = build_zip(&[("a//", b""), ("a/", b"")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::DuplicatePath))), "{result:?}");
    }

    #[test]
    fn explicit_directory_matching_an_implicit_ancestor_is_not_a_duplicate_d188() {
        // "a/b.txt" implicitly reserves "a" as a directory; a later explicit
        // "a/" entry for the same path is the normal archive form, not a
        // duplicate of itself.
        let entries = idx(&[("a/b.txt", b"x"), ("a/", b"")]);
        let paths: HashSet<_> = entries.iter().map(|e| (e.logical_path.clone(), e.is_dir)).collect();
        assert!(paths.contains(&("a".to_string(), true)));
    }

    #[test]
    fn disallowed_unix_mode_rejects_every_special_file_type_d188() {
        const S_IFLNK: u32 = 0o120000; // symlink
        const S_IFCHR: u32 = 0o020000; // character device
        const S_IFBLK: u32 = 0o060000; // block device
        const S_IFIFO: u32 = 0o010000; // FIFO
        const S_IFSOCK: u32 = 0o140000; // socket
        for special in [S_IFLNK, S_IFCHR, S_IFBLK, S_IFIFO, S_IFSOCK] {
            assert!(is_disallowed_unix_mode(special | 0o644), "mode {special:o} must be rejected");
        }
    }

    #[test]
    fn disallowed_unix_mode_allows_regular_files_directories_and_absent_mode_d188() {
        assert!(!is_disallowed_unix_mode(0o100644)); // S_IFREG
        assert!(!is_disallowed_unix_mode(0o040755)); // S_IFDIR
        assert!(!is_disallowed_unix_mode(0o644)); // no file-type bits at all
    }

    #[test]
    fn rejects_symlink_entries_through_the_real_archive_path_d188() {
        let buf = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buf);
        let options: zip::write::FileOptions<'_, '_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.add_symlink("link", "/etc/passwd", options).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::BadName(_, ZipNameError::SpecialEntry))), "{result:?}");
    }

    #[test]
    fn honors_cancellation_before_touching_the_archive_d188() {
        let bytes = build_zip(&[("a.txt", b"a")]);
        let result = index_zip(Cursor::new(bytes), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(true));
        assert_eq!(result, Err(ZipIndexError::Cancelled));
    }

    #[test]
    fn rejects_corrupt_bytes_boundedly_d188() {
        let result = index_zip(Cursor::new(b"not a zip file at all".to_vec()), &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false));
        assert!(matches!(result, Err(ZipIndexError::Open(_))));
    }
}

#[cfg(test)]
mod zip_extraction_tests_d188 {
    use super::*;

    fn write_zip_file(entries: &[(&str, &[u8])], at: &Path) {
        let file = std::fs::File::create(at).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, '_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, content).unwrap();
        }
        writer.finish().unwrap();
    }

    /// A `ZipMemberRef` for one logical path, as a real listing would build
    /// it (D-188 REJECT 5): archive length and physical index captured
    /// alongside size/CRC, the metadata these tests pass to
    /// `extract_zip_member`/`copy_zip_members_out` for revalidation.
    fn indexed_member(archive: &Path, internal: &str) -> ZipMemberRef {
        let file = std::fs::File::open(archive).unwrap();
        let archive_len = file.metadata().unwrap().len();
        let entries = index_zip(file, &DEFAULT_ZIP_LIMITS, &AtomicBool::new(false)).unwrap();
        let entry = entries.iter().find(|e| e.logical_path == internal).expect("member must be indexed");
        ZipMemberRef {
            archive: archive.to_path_buf(),
            internal: internal.to_string(),
            size: entry.size,
            crc32: entry.crc32,
            archive_len,
            member_index: entry.physical_index,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wf-zip-extract-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// D-240: a big plain-text file froze the window because the whole read
    /// went into one label for Pango to lay out. What is shown is capped, in
    /// lines and in bytes, whatever the file's size.
    #[test]
    fn text_preview_is_capped_in_bytes_and_lines() {
        let dir = scratch("preview-cap");

        // One enormous line: the byte cap is the only thing that can stop it.
        let wide = dir.join("wide.txt");
        std::fs::write(&wide, "x".repeat(400 * 1024)).unwrap();
        let shown = read_text_preview(&wide.to_string_lossy()).unwrap();
        assert!(shown.len() < 20 * 1024, "one long line not capped: {} bytes", shown.len());
        assert!(shown.ends_with("... (preview truncated)"), "no truncation note");

        // Many short lines: the line cap bites first.
        let tall = dir.join("tall.txt");
        std::fs::write(&tall, "line\n".repeat(5_000)).unwrap();
        let shown = read_text_preview(&tall.to_string_lossy()).unwrap();
        assert!(
            shown.lines().count() <= 302,
            "line cap not applied: {} lines",
            shown.lines().count()
        );

        // A small file is shown whole, with no note invented for it.
        let small = dir.join("small.txt");
        std::fs::write(&small, "two\nlines\n").unwrap();
        assert_eq!(read_text_preview(&small.to_string_lossy()).unwrap(), "two\nlines\n");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_zip_member_streams_exact_bytes_d188() {
        let dir = scratch("one");
        let archive = dir.join("a.zip");
        write_zip_file(&[("notes/todo.txt", b"buy milk")], &archive);
        let dest = dir.join("todo.txt");
        let member = indexed_member(&archive, "notes/todo.txt");
        extract_zip_member(&member, &dest, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"buy milk");
    }

    #[test]
    fn extract_zip_member_rejects_unknown_name_d188() {
        let dir = scratch("missing");
        let archive = dir.join("a.zip");
        write_zip_file(&[("a.txt", b"x")], &archive);
        let dest = dir.join("out.txt");
        let member = ZipMemberRef {
            archive: archive.clone(),
            internal: "not-there.txt".to_string(),
            size: 0,
            crc32: 0,
            archive_len: std::fs::metadata(&archive).unwrap().len(),
            member_index: 0,
        };
        assert!(extract_zip_member(&member, &dest, &AtomicBool::new(false)).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn extract_zip_member_rejects_a_changed_member_d188() {
        let dir = scratch("changed");
        let archive = dir.join("a.zip");
        write_zip_file(&[("a.txt", b"original")], &archive);
        let member = indexed_member(&archive, "a.txt");
        // Replace the archive's bytes for the same logical name after listing.
        write_zip_file(&[("a.txt", b"replaced bytes, different length")], &archive);
        let dest = dir.join("out.txt");
        let err = extract_zip_member(&member, &dest, &AtomicBool::new(false)).unwrap_err();
        assert!(err.contains("no longer matches what was listed"), "got: {err}");
        assert!(!dest.exists());
    }

    #[test]
    fn extract_zip_member_rejects_replacement_archive_preserving_name_size_crc_d188() {
        // D-188 REJECT 5: a replacement archive that happens to preserve the
        // selected member's name, size and CRC at a *different* position must
        // still be refused — archive identity (length) or exact entry
        // identity (physical index) must catch what size/CRC alone would miss.
        let dir = scratch("replaced-archive");
        let archive = dir.join("a.zip");
        write_zip_file(&[("decoy.txt", b"x"), ("a.txt", b"same-bytes")], &archive);
        let member = indexed_member(&archive, "a.txt");
        // Rebuild the archive with "a.txt" containing identical bytes (so size
        // and CRC are unchanged) but at a different physical position.
        write_zip_file(&[("a.txt", b"same-bytes"), ("decoy.txt", b"x")], &archive);
        let dest = dir.join("out.txt");
        let err = extract_zip_member(&member, &dest, &AtomicBool::new(false)).unwrap_err();
        assert!(err.contains("no longer matches what was listed"), "got: {err}");
        assert!(!dest.exists());
    }

    #[test]
    fn ensure_zip_open_cache_dir_is_mode_0700_d188() {
        let dir = scratch("cache");
        let cache = ensure_zip_open_cache_dir(&dir, "sess1", 1).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn ensure_zip_open_cache_dir_distinct_slots_never_collide_d188() {
        let dir = scratch("slots");
        let a = ensure_zip_open_cache_dir(&dir, "sess1", 1).unwrap();
        let b = ensure_zip_open_cache_dir(&dir, "sess1", 2).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn copy_zip_members_out_writes_every_file_d188() {
        let dir = scratch("copyout");
        let archive = dir.join("a.zip");
        write_zip_file(&[("x.txt", b"one"), ("y.txt", b"two")], &archive);
        let dest = dir.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let x = indexed_member(&archive, "x.txt");
        let y = indexed_member(&archive, "y.txt");
        let created = copy_zip_members_out(
            &archive,
            &[(x, "x.txt".to_string()), (y, "y.txt".to_string())],
            &dest,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(std::fs::read(dest.join("x.txt")).unwrap(), b"one");
        assert_eq!(std::fs::read(dest.join("y.txt")).unwrap(), b"two");
    }

    #[test]
    fn copy_zip_members_out_refuses_and_rolls_back_on_conflict_d188() {
        let dir = scratch("conflict");
        let archive = dir.join("a.zip");
        write_zip_file(&[("x.txt", b"one"), ("y.txt", b"two")], &archive);
        let dest = dir.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("y.txt"), b"already here").unwrap();
        let x = indexed_member(&archive, "x.txt");
        let y = indexed_member(&archive, "y.txt");
        let result = copy_zip_members_out(
            &archive,
            &[(x, "x.txt".to_string()), (y, "y.txt".to_string())],
            &dest,
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
        assert!(!dest.join("x.txt").exists());
        assert_eq!(std::fs::read(dest.join("y.txt")).unwrap(), b"already here");
    }

    /// The pre-existing-conflict test above proves rollback when the clash is
    /// visible at the start of the call. This proves the real race (D-188
    /// REJECT 4): nothing clashes at preflight time, something else creates
    /// the destination *after* every member has passed validation and before
    /// staging/publish — through the test-only `after_preflight` hook, which
    /// exists for exactly this proof and runs at no other time in production.
    #[test]
    fn copy_zip_members_out_refuses_destination_created_after_preflight_d188() {
        let dir = scratch("race");
        let archive = dir.join("a.zip");
        write_zip_file(&[("x.txt", b"one"), ("y.txt", b"two")], &archive);
        let dest = dir.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let x = indexed_member(&archive, "x.txt");
        let y = indexed_member(&archive, "y.txt");
        // Nothing exists yet, so preflight passes for both. The hook lands the
        // conflicting file at the exact moment after preflight completes and
        // before staging begins — the real check-then-use race, not a file
        // written before the call ever starts.
        let dest_for_hook = dest.clone();
        let result = copy_zip_members_out_for_test(
            &archive,
            &[(x, "x.txt".to_string()), (y, "y.txt".to_string())],
            &dest,
            &AtomicBool::new(false),
            &|| {
                std::fs::write(dest_for_hook.join("y.txt"), b"raced in").unwrap();
            },
        );
        assert!(result.is_err());
        assert!(!dest.join("x.txt").exists(), "owned output from this transaction must be rolled back");
        assert_eq!(std::fs::read(dest.join("y.txt")).unwrap(), b"raced in", "the raced object must be left untouched, not overwritten");
        let leftovers: Vec<_> = std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("-wf-zip-"))
            .collect();
        assert!(leftovers.is_empty(), "no owned stage may survive a failed transaction, found {leftovers:?}");
    }
}

#[cfg(test)]
mod drive_change_tests_d229 {
    use super::*;

    fn drive(name: &str, space: Option<Space>) -> Drive {
        Drive {
            name: name.to_string(),
            path: format!("/dev/{name}"),
            mountpoint: format!("/mnt/{name}"),
            size: 100,
            fstype: "ext4".into(),
            kind: "drive".into(),
            removable: false,
            space,
        }
    }

    /// D-229: a drive scan is the same, differs in space alone, or would change the rows.
    #[test]
    fn drive_change_tells_same_space_only_and_structural_apart_d229() {
        let some = |total, available| Some(Space { total, available });
        let a = drive("a", some(100, 25));
        let b = drive("b", None);
        let shown = vec![a.clone(), b.clone()];

        assert_eq!(drive_change(&[], &[]), DriveChange::Same);
        assert_eq!(drive_change(&shown, &shown.clone()), DriveChange::Same);

        // Space alone, in every direction it can move.
        for space in [some(100, 40), some(200, 25), some(0, 0), None] {
            let scanned = vec![Drive { space, ..a.clone() }, b.clone()];
            assert_eq!(drive_change(&shown, &scanned), DriveChange::SpaceOnly, "space {space:?}");
            assert_eq!(drive_change(&scanned, &shown), DriveChange::SpaceOnly, "space {space:?} back");
        }
        let appeared = vec![a.clone(), Drive { space: some(50, 25), ..b.clone() }];
        assert_eq!(drive_change(&shown, &appeared), DriveChange::SpaceOnly);

        // Every other field changes what a row shows or what its menu acts on.
        let other_fields: [(&str, Drive); 7] = [
            ("name", Drive { name: "z".into(), ..a.clone() }),
            ("path", Drive { path: "/dev/z".into(), ..a.clone() }),
            ("mountpoint", Drive { mountpoint: String::new(), ..a.clone() }),
            ("size", Drive { size: 200, ..a.clone() }),
            ("fstype", Drive { fstype: "ntfs".into(), ..a.clone() }),
            ("kind", Drive { kind: "optical".into(), ..a.clone() }),
            ("removable", Drive { removable: true, ..a.clone() }),
        ];
        for (field, changed) in other_fields {
            let scanned = vec![changed.clone(), b.clone()];
            assert_eq!(drive_change(&shown, &scanned), DriveChange::Structural, "{field}");
            // A change in space on top does not make it a space-only change.
            let scanned = vec![Drive { space: some(100, 1), ..changed }, b.clone()];
            assert_eq!(drive_change(&shown, &scanned), DriveChange::Structural, "{field} with space");
        }

        // Order, arrival and removal.
        assert_eq!(drive_change(&shown, &[b.clone(), a.clone()]), DriveChange::Structural);
        assert_eq!(drive_change(&shown, &[a.clone(), b.clone(), drive("c", None)]), DriveChange::Structural);
        assert_eq!(drive_change(&shown, &[a.clone()]), DriveChange::Structural);
        assert_eq!(drive_change(&shown, &[]), DriveChange::Structural);
        assert_eq!(drive_change(&[], &shown), DriveChange::Structural);
    }
}

#[cfg(test)]
mod extract_here_tests_d230 {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    /// Extract Here as the app runs it, reduced to the names it says it extracted
    /// and the names it says it skipped (true marks a whole folder). Its own
    /// function so the same test text also builds on a tree from before the
    /// repair, where the old route reports nothing.
    pub(super) fn extract_here_d230(archive: &Path, dest: &Path) -> Result<(Vec<String>, Vec<(String, bool)>), String> {
        extract_archive_here(archive.to_str().unwrap(), dest.to_str().unwrap())
            .map(|done| (done.published, done.skipped.into_iter().map(|skip| (skip.name, skip.folder)).collect()))
    }

    /// A folder of this test's own under the scratch root, made new and removed on
    /// drop, also when an assertion fails. Anything a case made read-only is
    /// opened up first so nothing is left behind.
    pub(super) struct Owned(pub(super) PathBuf);

    impl Drop for Owned {
        fn drop(&mut self) {
            open_up(&self.0);
            let _ = fs::remove_dir_all(&self.0);
            if let Some(root) = self.0.parent() {
                let _ = fs::remove_dir(root); // fails unless empty
            }
        }
    }

    pub(super) fn owned(tag: &str) -> Owned {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("wf-d230-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        // Another test's cleanup may remove the shared root while it is empty.
        for _ in 0..50 {
            let dir = scratch_root().join(&name);
            match fs::create_dir(&dir) {
                Ok(()) => return Owned(dir),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => panic!("a new folder of this test's own, {}: {e}", dir.display()),
            }
        }
        panic!("the scratch root kept disappearing");
    }

    /// Owner access back on every real folder under `dir`; links are not followed.
    pub(super) fn open_up(dir: &Path) {
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    open_up(&entry.path());
                }
            }
        }
    }

    /// Every entry under `dir` by relative path, sorted: a folder ends in '/', a
    /// link shows its target, a file its bytes (a count when they are not short
    /// text). Links are never followed.
    pub(super) fn tree(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path.strip_prefix(base).unwrap().display().to_string();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.file_type().is_symlink() {
                    out.push(format!("{rel} -> {}", fs::read_link(&path).unwrap().display()));
                } else if meta.is_dir() {
                    out.push(format!("{rel}/"));
                    walk(base, &path, out);
                } else {
                    let bytes = fs::read(&path).unwrap();
                    match std::str::from_utf8(&bytes) {
                        Ok(text) if bytes.len() <= 80 => out.push(format!("{rel} = {text}")),
                        _ => out.push(format!("{rel} = <{} bytes>", bytes.len())),
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// Packs `names` from `src` with the real tool for `kind`.
    pub(super) fn pack(kind: &str, src: &Path, names: &[&str], archive: &Path) {
        let (tool, lead): (&str, &[&str]) = match kind {
            "zip" => ("zip", &["-r", "-q"]),
            "tar" => ("tar", &["-cf"]),
            "7z" => ("7z", &["a"]),
            other => panic!("no packer for {other}"),
        };
        let out = Command::new(tool)
            .args(lead)
            .arg(archive)
            .args(names)
            .current_dir(src)
            .output()
            .expect("run the packer");
        assert!(out.status.success(), "{tool} could not pack: {}", String::from_utf8_lossy(&out.stderr));
    }

    const OLD_NOTE: &str = "OLD note: the user's own bytes";
    const OLD_KEEP: &str = "OLD keep";
    const OLD_OWN: &str = "OLD own";
    const BYSTANDER: &str = "not in the archive";
    const OUTSIDE: &str = "outside sentinel";

    /// The top-level names every archive below holds.
    pub(super) const PACKED: [&str; 6] = ["dangling", "fresh-dir", "fresh.txt", "link-name", "note.txt", "photos"];

    pub(super) fn write_source(src: &Path) {
        fs::create_dir(src).unwrap();
        fs::write(src.join("note.txt"), "NEW note").unwrap();
        fs::write(src.join("link-name"), "NEW link-name").unwrap();
        fs::write(src.join("dangling"), "NEW dangling").unwrap();
        fs::create_dir(src.join("photos")).unwrap();
        fs::write(src.join("photos/new.jpg"), "NEW photo").unwrap();
        fs::write(src.join("photos/keep.txt"), "NEW keep").unwrap();
        fs::write(src.join("fresh.txt"), "fresh file").unwrap();
        fs::create_dir(src.join("fresh-dir")).unwrap();
        fs::write(src.join("fresh-dir/inner.txt"), "fresh inner").unwrap();
    }

    fn id(path: &Path) -> (u64, u64) {
        let meta = fs::symlink_metadata(path).unwrap();
        (meta.dev(), meta.ino())
    }

    /// A destination that already holds a file, a link, a dangling link and a
    /// folder under names the archive also has, plus a file it does not have.
    fn clashes_are_kept_and_reported(kind: &str) {
        let root = owned(&format!("clash-{kind}"));
        let (src, dest, outside) = (root.0.join("src"), root.0.join("dest"), root.0.join("outside-sentinel.txt"));
        write_source(&src);
        fs::write(&outside, OUTSIDE).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("note.txt"), OLD_NOTE).unwrap();
        symlink(&outside, dest.join("link-name")).unwrap();
        symlink("no-such-target", dest.join("dangling")).unwrap();
        fs::create_dir(dest.join("photos")).unwrap();
        fs::write(dest.join("photos/keep.txt"), OLD_KEEP).unwrap();
        fs::write(dest.join("photos/own.txt"), OLD_OWN).unwrap();
        fs::write(dest.join("bystander.txt"), BYSTANDER).unwrap();
        let archive = dest.join(format!("pack.{kind}"));
        pack(kind, &src, &PACKED, &archive);
        let archive_bytes = fs::read(&archive).unwrap();
        let before = tree(&dest);
        let kept = ["note.txt", "link-name", "dangling", "photos", "photos/keep.txt", "photos/own.txt", "bystander.txt"];
        let ids: Vec<(u64, u64)> = kept.iter().map(|name| id(&dest.join(name))).collect();

        let result = extract_here_d230(&archive, &dest);

        // The user's own bytes first: this is what the old route replaced.
        assert_eq!(fs::read_to_string(dest.join("note.txt")).unwrap(), OLD_NOTE, "{kind}: the existing note.txt was replaced");
        // An existing link stays that link, and nothing is written through it.
        assert_eq!(fs::read_link(dest.join("link-name")).unwrap(), outside, "{kind}: link-name is no longer the same link");
        assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE, "{kind}: the file behind link-name was changed");
        assert_eq!(
            fs::read_link(dest.join("dangling")).unwrap(),
            Path::new("no-such-target"),
            "{kind}: the dangling link was replaced"
        );
        assert!(
            fs::symlink_metadata(dest.join("no-such-target")).is_err(),
            "{kind}: something was written through the dangling link"
        );
        // The existing folder is exactly as it was: nothing merged in, nothing replaced.
        assert_eq!(
            tree(&dest.join("photos")),
            [format!("keep.txt = {OLD_KEEP}"), format!("own.txt = {OLD_OWN}")],
            "{kind}: the existing photos folder"
        );
        assert_eq!(fs::read_to_string(dest.join("bystander.txt")).unwrap(), BYSTANDER);
        for (name, before) in kept.iter().zip(&ids) {
            assert_eq!(id(&dest.join(name)), *before, "{kind}: {name} is not the same entry as before");
        }
        // What does not clash is extracted as usual.
        assert_eq!(fs::read_to_string(dest.join("fresh.txt")).unwrap(), "fresh file");
        assert_eq!(tree(&dest.join("fresh-dir")), ["inner.txt = fresh inner"]);
        // And the result says so, name by name; the folder is marked as skipped whole.
        let (published, skipped) = result.unwrap_or_else(|err| panic!("{kind}: {err}"));
        assert_eq!(published, ["fresh-dir", "fresh.txt"], "{kind}: extracted");
        let skipped: Vec<(&str, bool)> = skipped.iter().map(|(name, folder)| (name.as_str(), *folder)).collect();
        assert_eq!(
            skipped,
            [("dangling", false), ("link-name", false), ("note.txt", false), ("photos", true)],
            "{kind}: skipped"
        );
        // The archive is untouched and nothing else was added, hidden or not.
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes, "{kind}: the archive changed");
        let mut expected = before.clone();
        expected.extend([
            "fresh-dir/".to_string(),
            "fresh-dir/inner.txt = fresh inner".to_string(),
            "fresh.txt = fresh file".to_string(),
        ]);
        expected.sort();
        assert_eq!(tree(&dest), expected, "{kind}: the folder afterwards");
    }

    /// D-230: Extract Here leaves every existing entry as it was and names what it
    /// skipped. One test per archive type, each packed by the real tool.
    #[test]
    fn extract_here_keeps_existing_entries_zip_d230() {
        clashes_are_kept_and_reported("zip");
    }

    #[test]
    fn extract_here_keeps_existing_entries_tar_d230() {
        clashes_are_kept_and_reported("tar");
    }

    #[test]
    fn extract_here_keeps_existing_entries_7z_d230() {
        clashes_are_kept_and_reported("7z");
    }

    /// With nothing in the way, every entry arrives with its bytes and nothing
    /// else is left in the folder.
    fn empty_destination_gets_everything(kind: &str) {
        let root = owned(&format!("empty-{kind}"));
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        write_source(&src);
        fs::create_dir(&dest).unwrap();
        let archive = dest.join(format!("pack.{kind}"));
        pack(kind, &src, &PACKED, &archive);
        let archive_bytes = fs::read(&archive).unwrap();

        let result = extract_here_d230(&archive, &dest);

        assert!(result.is_ok(), "{kind}: {result:?}");
        let mut expected = tree(&src);
        expected.push(format!("pack.{kind} = <{} bytes>", archive_bytes.len()));
        expected.sort();
        assert_eq!(tree(&dest), expected, "{kind}: every entry, and nothing else");
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes, "{kind}: the archive changed");
    }

    #[test]
    fn extract_here_into_an_empty_folder_zip_d230() {
        empty_destination_gets_everything("zip");
    }

    #[test]
    fn extract_here_into_an_empty_folder_tar_d230() {
        empty_destination_gets_everything("tar");
    }

    #[test]
    fn extract_here_into_an_empty_folder_7z_d230() {
        empty_destination_gets_everything("7z");
    }
}

#[cfg(test)]
mod no_replace_move_wording_d230 {
    use super::*;

    /// D-230 leaves the shared no-replace move as it was for Trash and transfers:
    /// the same three error texts, on a tree from before the repair and after it.
    #[test]
    fn no_replace_move_keeps_its_error_texts_d230() {
        let dir = scratch_root().join(format!("wf-d230-wording-{}", std::process::id()));
        fs::create_dir(&dir).expect("a new folder of this test's own");
        struct Gone(PathBuf);
        impl Drop for Gone {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _gone = Gone(dir.clone());
        let (a, b, missing) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("missing.txt"));
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();

        assert_eq!(rename_no_replace(&a, &b), Err(format!("already exists: {}", b.display())));
        assert_eq!((fs::read_to_string(&a).unwrap(), fs::read_to_string(&b).unwrap()), ("A".to_string(), "B".to_string()));
        let target = dir.join("c.txt");
        assert_eq!(
            rename_no_replace(&missing, &target),
            Err(format!("{}: {}", target.display(), std::io::Error::from_raw_os_error(libc::ENOENT)))
        );
        let nul = dir.join("has\0nul");
        assert_eq!(rename_no_replace(&a, &nul), Err(format!("path contains a NUL byte: {}", nul.display())));
        assert_eq!(rename_no_replace(&a, &target), Ok(()));
        assert_eq!(fs::read_to_string(&target).unwrap(), "A");
    }
}

#[cfg(test)]
mod extract_here_seam_tests_d230 {
    use super::extract_here_tests_d230::{owned, pack, tree, write_source, PACKED};
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn text(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    fn skip(name: &str, folder: bool) -> ExtractSkip {
        ExtractSkip { name: name.to_string(), folder }
    }

    fn sorted(mut lines: Vec<String>) -> Vec<String> {
        lines.sort();
        lines
    }

    /// `name = <n bytes>`, the way `tree` shows an archive.
    fn sized(archive: &Path) -> String {
        format!(
            "{} = <{} bytes>",
            archive.file_name().unwrap().to_string_lossy(),
            fs::metadata(archive).unwrap().len()
        )
    }

    /// The temporary folders of Extract Here that are directly in `dir`.
    fn stages(dir: &Path) -> Vec<String> {
        sorted(
            fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(".wf-extract-"))
                .collect(),
        )
    }

    /// The folder a tool was told to write into.
    fn out_dir(cmd: &str, args: &[&str]) -> PathBuf {
        let after = |flag: &str| args.iter().position(|arg| *arg == flag).map(|i| args[i + 1]);
        let dir = match cmd {
            "tar" => after("-C"),
            "unzip" => after("-d"),
            "wayfinder-zip" => args.get(1).copied(),
            "unar" => after("-output-directory"),
            "7z" => args.iter().find_map(|arg| arg.strip_prefix("-o")),
            _ => None,
        };
        PathBuf::from(dir.unwrap_or_else(|| panic!("no output folder in {cmd} {args:?}")))
    }

    /// With nothing in the way the result lists every entry, for each archive
    /// type, and only then is the message the plain one.
    #[test]
    fn a_clean_result_lists_every_entry_d230() {
        let root = owned("clean");
        let src = root.0.join("src");
        write_source(&src);
        for kind in ["zip", "tar", "7z"] {
            let dest = root.0.join(format!("dest-{kind}"));
            fs::create_dir(&dest).unwrap();
            let archive = dest.join(format!("pack.{kind}"));
            pack(kind, &src, &PACKED, &archive);
            let done = extract_archive_here(text(&archive), text(&dest)).unwrap();
            assert_eq!(done, ExtractHereOutcome { published: names(&PACKED), ..Default::default() }, "{kind}");
            assert_eq!(done.summary(), "Extracted archive", "{kind}");
            assert!(stages(&dest).is_empty(), "{kind}");
        }
    }

    /// A tool that fails part-way leaves nothing in what the next tool's output is
    /// taken from. A zip: unzip fails after it wrote, the real 7z follows, and a
    /// name that is taken stays out of reach of 7z's own overwrite switch.
    #[test]
    fn a_failed_tool_leaves_nothing_in_the_result_zip_d230() {
        let root = owned("fallback-zip");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        write_source(&src);
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("note.txt"), "OLD note").unwrap();
        let archive = dest.join("pack.zip");
        pack("zip", &src, &["fresh.txt", "note.txt"], &archive);
        let used = RefCell::new(Vec::new());
        let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
            let out = out_dir(cmd, args);
            used.borrow_mut().push((cmd.to_string(), out.clone()));
            if cmd == "wayfinder-zip" {
                fs::write(out.join("JUNK.txt"), "left by the failed tool").unwrap();
                fs::write(out.join("fresh.txt"), "HALF").unwrap();
                return Err("wayfinder-zip failed on purpose".to_string());
            }
            run_cmd_capture(cmd, args)
        };

        let done = extract_here_with(text(&archive), text(&dest), &run, &rename_no_replace_io).unwrap();

        assert_eq!(
            done,
            ExtractHereOutcome {
                published: names(&["fresh.txt"]),
                skipped: vec![skip("note.txt", false)],
                ..Default::default()
            }
        );
        assert_eq!(
            tree(&dest),
            sorted(vec!["fresh.txt = fresh file".to_string(), "note.txt = OLD note".to_string(), sized(&archive)]),
            "the second tool's output alone, and the taken name as it was"
        );
        // Each tool wrote into a folder of its own inside the temporary folder, never the destination.
        let used = used.into_inner();
        assert_eq!(used.iter().map(|(cmd, _)| cmd.as_str()).collect::<Vec<_>>(), ["wayfinder-zip", "7z"]);
        assert_ne!(used[0].1, used[1].1, "each attempt gets its own folder");
        for (cmd, out) in &used {
            let stage = out.parent().unwrap();
            assert_eq!(stage.parent(), Some(dest.as_path()), "{cmd} wrote into {}", out.display());
            assert!(stage.file_name().unwrap().to_string_lossy().starts_with(".wf-extract-"), "{cmd}: {}", out.display());
        }
    }

    /// The same for a .7z: 7z fails after it wrote and the real unar follows.
    /// Once with the name free, and once with it taken, where unar's own
    /// overwrite switch must not reach the existing file.
    #[test]
    fn a_failed_tool_leaves_nothing_in_the_result_7z_d230() {
        let root = owned("fallback-7z");
        let src = root.0.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("solo.txt"), "solo from the archive").unwrap();
        for taken in [false, true] {
            let dest = root.0.join(if taken { "dest-taken" } else { "dest-free" });
            fs::create_dir(&dest).unwrap();
            if taken {
                fs::write(dest.join("solo.txt"), "OLD solo").unwrap();
            }
            let archive = dest.join("one.7z");
            pack("7z", &src, &["solo.txt"], &archive);
            let tools = RefCell::new(Vec::new());
            let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
                tools.borrow_mut().push(cmd.to_string());
                if cmd == "7z" {
                    let out = out_dir(cmd, args);
                    fs::write(out.join("JUNK.txt"), "left by the failed tool").unwrap();
                    fs::write(out.join("solo.txt"), "HALF").unwrap();
                    return Err("7z failed on purpose".to_string());
                }
                run_cmd_capture(cmd, args)
            };

            let done = extract_here_with(text(&archive), text(&dest), &run, &rename_no_replace_io).unwrap();

            assert_eq!(tools.into_inner(), ["7z", "unar"], "taken={taken}");
            let solo = if taken {
                assert_eq!(done, ExtractHereOutcome { skipped: vec![skip("solo.txt", false)], ..Default::default() });
                "OLD solo"
            } else {
                assert_eq!(done, ExtractHereOutcome { published: names(&["solo.txt"]), ..Default::default() });
                "solo from the archive"
            };
            assert_eq!(tree(&dest), sorted(vec![sized(&archive), format!("solo.txt = {solo}")]), "taken={taken}");
        }
    }

    /// When every tool fails, nothing is moved and the folder is as it was,
    /// whatever the tools wrote first. The error is the one the old route gave.
    #[test]
    fn nothing_is_moved_when_every_tool_fails_d230() {
        let root = owned("all-fail");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        write_source(&src);
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("note.txt"), "OLD note").unwrap();
        for (kind, error) in [
            ("zip", "7z failed on purpose"),
            ("7z", "7z failed on purpose; unar failed on purpose"),
            ("tar", "tar failed on purpose"),
        ] {
            let archive = dest.join(format!("pack.{kind}"));
            pack(kind, &src, &PACKED, &archive);
            let before = tree(&dest);
            let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
                fs::write(out_dir(cmd, args).join("note.txt"), "written by a tool that then failed").unwrap();
                Err(format!("{cmd} failed on purpose"))
            };

            let result = extract_here_with(text(&archive), text(&dest), &run, &rename_no_replace_io);

            assert_eq!(result, Err(error.to_string()), "{kind}");
            assert_eq!(tree(&dest), before, "{kind}: the folder is as it was");
        }
    }

    /// Real damaged archives, real tools. What a tool wrote before it gave up
    /// (the new bytes for a name that is taken among it) never reaches the folder.
    #[test]
    fn a_damaged_archive_moves_nothing_d230() {
        let root = owned("damaged");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        fs::create_dir(&src).unwrap();
        fs::write(src.join("note.txt"), "NEW note").unwrap();
        // Bytes that do not compress, so cutting the archive short cuts this entry.
        let mut x = 0x2545_F491_4F6C_DD1D_u64;
        let noise: Vec<u8> = (0..64 * 1024)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        fs::write(src.join("z-big.bin"), noise).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("note.txt"), "OLD note").unwrap();
        let cut = |archive: &Path, len: u64| {
            assert!(fs::metadata(archive).unwrap().len() > 60_000, "{} is too small to cut", archive.display());
            fs::OpenOptions::new().write(true).open(archive).unwrap().set_len(len).unwrap();
        };
        // Cut inside the second entry: tar writes note.txt and part of the next, then fails.
        let cut_tar = dest.join("cut.tar");
        pack("tar", &src, &["note.txt", "z-big.bin"], &cut_tar);
        cut(&cut_tar, 4096);
        // Cut the same way: Wayfinder's own reader refuses it, and the 7z that follows writes what it can, then fails.
        let cut_zip = dest.join("cut.zip");
        pack("zip", &src, &["note.txt", "z-big.bin"], &cut_zip);
        cut(&cut_zip, 3000);
        // Not an archive at all: 7z and unar both refuse it.
        let junk = dest.join("junk.7z");
        fs::write(&junk, b"this is not an archive at all\n".repeat(64)).unwrap();
        let before = tree(&dest);

        let cases: [(&Path, &[(&str, Option<&str>)]); 3] = [
            (&cut_tar, &[("tar", Some("NEW note"))]),
            (&cut_zip, &[("wayfinder-zip", None), ("7z", Some("NEW note"))]),
            (&junk, &[("7z", None), ("unar", None)]),
        ];
        for (archive, expected) in cases {
            // Each real tool, and the note.txt it left in its own folder when it stopped.
            let wrote = RefCell::new(Vec::new());
            let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
                let result = run_cmd_capture(cmd, args);
                assert!(result.is_err(), "{cmd} accepted {}", archive.display());
                wrote
                    .borrow_mut()
                    .push((cmd.to_string(), fs::read_to_string(out_dir(cmd, args).join("note.txt")).ok()));
                result
            };

            let result = extract_here_with(text(archive), text(&dest), &run, &rename_no_replace_io);

            assert!(result.is_err(), "{}: {result:?}", archive.display());
            let wrote = wrote.into_inner();
            let wrote: Vec<(&str, Option<&str>)> = wrote.iter().map(|(cmd, note)| (cmd.as_str(), note.as_deref())).collect();
            assert_eq!(wrote, expected, "{}", archive.display());
            assert_eq!(tree(&dest), before, "{}: the folder is as it was", archive.display());
        }
    }

    /// A name that is free when the move is decided and taken when it happens is
    /// kept all the same: the move itself refuses, and the entry is reported as
    /// skipped. A file and a folder.
    #[test]
    fn a_name_taken_at_the_last_moment_is_kept_d230() {
        let root = owned("late");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        write_source(&src);
        fs::create_dir(&dest).unwrap();
        let archive = dest.join("pack.tar");
        pack("tar", &src, &PACKED, &archive);
        // Just before the real move of these two names, something else takes them.
        let publish = |from: &Path, to: &Path| -> std::io::Result<()> {
            match to.file_name().and_then(|name| name.to_str()) {
                Some("fresh.txt") => fs::write(to, "ARRIVED FIRST").unwrap(),
                Some("fresh-dir") => {
                    fs::create_dir(to).unwrap();
                    fs::write(to.join("theirs.txt"), "ARRIVED FIRST").unwrap();
                }
                _ => {}
            }
            rename_no_replace_io(from, to)
        };

        let done = extract_here_with(text(&archive), text(&dest), &run_cmd_capture, &publish).unwrap();

        assert_eq!(
            done,
            ExtractHereOutcome {
                published: names(&["dangling", "link-name", "note.txt", "photos"]),
                skipped: vec![skip("fresh-dir", true), skip("fresh.txt", false)],
                ..Default::default()
            }
        );
        assert_eq!(fs::read_to_string(dest.join("fresh.txt")).unwrap(), "ARRIVED FIRST");
        assert_eq!(tree(&dest.join("fresh-dir")), ["theirs.txt = ARRIVED FIRST"]);
        assert_eq!(
            done.summary(),
            "Extracted 4 items. Skipped 2 that already exist: fresh-dir (whole folder), fresh.txt"
        );
        assert!(stages(&dest).is_empty());
    }

    /// A move that fails for a real reason is that entry's alone: what was moved
    /// before it stays and is reported, and the entries after it are still moved.
    #[test]
    fn a_failed_move_keeps_the_other_results_d230() {
        let root = owned("partial");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        fs::create_dir(&src).unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "note.txt"] {
            fs::write(src.join(name), format!("new {name}")).unwrap();
        }
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("note.txt"), "OLD note").unwrap();
        let archive = dest.join("pack.zip");
        pack("zip", &src, &["a.txt", "b.txt", "c.txt", "note.txt"], &archive);
        let publish = |from: &Path, to: &Path| -> std::io::Result<()> {
            if to.file_name().is_some_and(|name| name == "b.txt") {
                return Err(std::io::Error::from_raw_os_error(libc::EIO));
            }
            rename_no_replace_io(from, to)
        };

        let done = extract_here_with(text(&archive), text(&dest), &run_cmd_capture, &publish).unwrap();

        let why = std::io::Error::from_raw_os_error(libc::EIO).to_string();
        assert_eq!(
            done,
            ExtractHereOutcome {
                published: names(&["a.txt", "c.txt"]),
                skipped: vec![skip("note.txt", false)],
                failed: vec![("b.txt".to_string(), why.clone())],
                leftover: None,
            }
        );
        assert_eq!(
            done.summary(),
            format!("Extracted 2 items. Skipped 1 that already exists: note.txt. Failed on 1: b.txt ({why})")
        );
        assert_eq!(
            tree(&dest),
            sorted(vec![
                "a.txt = new a.txt".to_string(),
                "c.txt = new c.txt".to_string(),
                "note.txt = OLD note".to_string(),
                sized(&archive),
            ]),
            "the two that moved, the taken name as it was, and no temporary folder"
        );
    }

    /// When the temporary folder cannot be removed the result says where it is,
    /// after a good extraction and after a failed one. What was moved stays moved.
    #[test]
    fn a_temporary_folder_left_behind_is_reported_d230() {
        let root = owned("leftover");
        let src = root.0.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("b.txt"), "b").unwrap();
        let denied = std::io::Error::from_raw_os_error(libc::EACCES).to_string();
        let lock = |dir: &Path, mode: u32| fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
        {
            let dest = root.0.join("dest-good");
            fs::create_dir(&dest).unwrap();
            let archive = dest.join("pack.tar");
            pack("tar", &src, &["a.txt", "b.txt"], &archive);
            // Once the last entry has moved the folder stops accepting changes, so
            // the temporary folder cannot be taken out of it.
            let publish = |from: &Path, to: &Path| -> std::io::Result<()> {
                let moved = rename_no_replace_io(from, to);
                if to.file_name().is_some_and(|name| name == "b.txt") {
                    lock(&dest, 0o555);
                }
                moved
            };

            let done = extract_here_with(text(&archive), text(&dest), &run_cmd_capture, &publish).unwrap();
            lock(&dest, 0o755);

            let found = stages(&dest);
            assert_eq!(found.len(), 1, "the temporary folder is still there: {found:?}");
            let left = dest.join(&found[0]).display().to_string();
            assert_eq!(
                done,
                ExtractHereOutcome {
                    published: names(&["a.txt", "b.txt"]),
                    leftover: Some((left.clone(), denied.clone())),
                    ..Default::default()
                }
            );
            assert_eq!(done.summary(), format!("Extracted 2 items. Temporary folder not removed: {left} ({denied})"));
            assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "a");
            assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "b");
        }
        {
            let dest = root.0.join("dest-failed");
            fs::create_dir(&dest).unwrap();
            let archive = dest.join("pack.tar");
            pack("tar", &src, &["a.txt", "b.txt"], &archive);
            let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
                fs::write(out_dir(cmd, args).join("a.txt"), "HALF").unwrap();
                lock(&dest, 0o555);
                Err(format!("{cmd} failed on purpose"))
            };

            let result = extract_here_with(text(&archive), text(&dest), &run, &rename_no_replace_io);
            lock(&dest, 0o755);

            let found = stages(&dest);
            assert_eq!(found.len(), 1, "the temporary folder is still there: {found:?}");
            let left = dest.join(&found[0]).display().to_string();
            assert_eq!(
                result,
                Err(format!("tar failed on purpose. Temporary folder not removed: {left} ({denied})"))
            );
            let top: Vec<String> = fs::read_dir(&dest)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(sorted(top), [found[0].as_str(), "pack.tar"], "nothing was moved into the folder");
        }
    }

    /// Cleanup removes the folder this extraction made and no other. When that
    /// folder was moved away, with or without another put under its name, nothing
    /// is removed and the result says so.
    #[test]
    fn cleanup_never_removes_a_folder_it_did_not_make_d230() {
        let root = owned("swap");
        let src = root.0.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        for (replaced, why) in [(true, "something else now has its name"), (false, "it was moved or removed by something else")] {
            let dest = root.0.join(format!("dest-{replaced}"));
            fs::create_dir(&dest).unwrap();
            let archive = dest.join("pack.tar");
            pack("tar", &src, &["a.txt"], &archive);
            let moved_away = root.0.join(format!("moved-away-{replaced}"));
            let publish = |from: &Path, to: &Path| -> std::io::Result<()> {
                let moved = rename_no_replace_io(from, to);
                let stage = from.parent().unwrap().parent().unwrap();
                fs::rename(stage, &moved_away).unwrap();
                if replaced {
                    fs::create_dir(stage).unwrap();
                    fs::write(stage.join("theirs.txt"), "NOT OURS").unwrap();
                }
                moved
            };

            let done = extract_here_with(text(&archive), text(&dest), &run_cmd_capture, &publish).unwrap();

            let (left, reason) = done.leftover.clone().expect("the result says the folder was not removed");
            assert_eq!(reason, why);
            assert_eq!(done.published, ["a.txt"]);
            assert_eq!(Path::new(&left).parent(), Some(dest.as_path()));
            if replaced {
                assert_eq!(tree(Path::new(&left)), ["theirs.txt = NOT OURS"], "the other folder is untouched");
            } else {
                assert!(fs::symlink_metadata(&left).is_err());
            }
            assert!(moved_away.is_dir(), "the folder that was moved away was not followed");
            assert_eq!(done.summary(), format!("Extracted 1 item. Temporary folder not removed: {left} ({why})"));
        }
    }

    /// The temporary folder is always made new. Names it would have used that are
    /// taken, by a dangling link, a file or a folder with something in it, are
    /// passed over, and are all still there, untouched, afterwards.
    #[test]
    fn a_taken_temporary_name_is_never_used_d230() {
        let root = owned("reserve");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::create_dir(&dest).unwrap();
        let archive = dest.join("pack.tar");
        pack("tar", &src, &["a.txt"], &archive);
        let pid = std::process::id();
        // Other tests may use a few numbers meanwhile; this run still starts inside the taken range.
        let first = EXTRACT_STAGE_COUNTER.load(Ordering::SeqCst);
        for n in first..first + 300 {
            let name = dest.join(format!(".wf-extract-{pid}-{n}"));
            match n % 3 {
                0 => symlink("nowhere", &name).unwrap(),
                1 => fs::write(&name, "a file with that name").unwrap(),
                _ => {
                    fs::create_dir(&name).unwrap();
                    fs::write(name.join("theirs.txt"), "NOT OURS").unwrap();
                }
            }
        }
        let before = tree(&dest);

        let done = extract_archive_here(text(&archive), text(&dest)).unwrap();

        assert_eq!(done, ExtractHereOutcome { published: names(&["a.txt"]), ..Default::default() });
        assert!(
            EXTRACT_STAGE_COUNTER.load(Ordering::SeqCst) > first + 300,
            "the run had to pass over taken names"
        );
        let mut expected = before;
        expected.push("a.txt = a".to_string());
        assert_eq!(tree(&dest), sorted(expected), "every taken name is as it was, and nothing was left behind");
    }

    /// A folder the archive marks read-only arrives with that mode. When its name
    /// is taken it is skipped whole and still cleaned out of the temporary folder.
    #[test]
    fn a_read_only_folder_is_extracted_or_skipped_whole_d230() {
        let root = owned("read-only");
        let src = root.0.join("src");
        fs::create_dir_all(src.join("locked")).unwrap();
        fs::write(src.join("locked/inner.txt"), "inner").unwrap();
        fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
        {
            let dest = root.0.join("dest-free");
            fs::create_dir(&dest).unwrap();
            let archive = dest.join("pack.tar");
            pack("tar", &src, &["locked"], &archive);
            // What tar itself gives the folder, for comparison.
            let plain = root.0.join("plain");
            fs::create_dir(&plain).unwrap();
            extract_archive(text(&archive), text(&plain)).unwrap();
            assert_eq!(mode(&plain.join("locked")) & 0o200, 0, "the folder really is read-only");

            let done = extract_archive_here(text(&archive), text(&dest)).unwrap();

            assert_eq!(done, ExtractHereOutcome { published: names(&["locked"]), ..Default::default() });
            assert_eq!(mode(&dest.join("locked")), mode(&plain.join("locked")), "the folder keeps the archive's mode");
            assert_eq!(tree(&dest.join("locked")), ["inner.txt = inner"]);
            assert!(stages(&dest).is_empty());
        }
        {
            let dest = root.0.join("dest-taken");
            fs::create_dir_all(dest.join("locked")).unwrap();
            fs::write(dest.join("locked/own.txt"), "OLD own").unwrap();
            let archive = dest.join("pack.tar");
            pack("tar", &src, &["locked"], &archive);

            let done = extract_archive_here(text(&archive), text(&dest)).unwrap();

            assert_eq!(done, ExtractHereOutcome { skipped: vec![skip("locked", true)], ..Default::default() });
            assert_eq!(done.summary(), "Nothing extracted. Skipped 1 that already exists: locked (whole folder)");
            assert_eq!(tree(&dest.join("locked")), ["own.txt = OLD own"]);
            assert!(stages(&dest).is_empty(), "the read-only folder was removed with the temporary folder");
        }
    }

    /// A tar made of "." carries that folder's own mode, and tar gives it to the
    /// folder it writes into. That is the temporary folder now, so a read-only
    /// one neither blocks the result nor changes the mode of the folder on show.
    #[test]
    fn a_tar_of_a_read_only_folder_itself_is_extracted_d230() {
        let root = owned("dot");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("sub/b.txt"), "b").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o555)).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("a.txt"), "OLD a").unwrap();
        let archive = dest.join("pack.tar");
        pack("tar", &src, &["."], &archive);
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
        let before = mode(&dest);
        // tar really does this: decoding straight into a folder leaves it read-only.
        let plain = root.0.join("plain");
        fs::create_dir(&plain).unwrap();
        extract_archive(text(&archive), text(&plain)).unwrap();
        assert_eq!(mode(&plain) & 0o222, 0, "tar gave its output folder the archived mode");

        let done = extract_archive_here(text(&archive), text(&dest)).unwrap();

        assert_eq!(
            done,
            ExtractHereOutcome {
                published: names(&["sub"]),
                skipped: vec![skip("a.txt", false)],
                ..Default::default()
            }
        );
        assert_eq!(mode(&dest), before, "the folder on show keeps its own mode");
        assert_eq!(
            tree(&dest),
            sorted(vec![
                "a.txt = OLD a".to_string(),
                sized(&archive),
                "sub/".to_string(),
                "sub/b.txt = b".to_string(),
            ])
        );
    }

    /// Links inside the archive arrive as the same links, and a link whose name is
    /// taken is skipped as a name, not as a folder, whatever it points at.
    #[test]
    fn links_in_the_archive_stay_links_d230() {
        let root = owned("links");
        let (src, dest) = (root.0.join("src"), root.0.join("dest"));
        fs::create_dir_all(src.join("real-dir")).unwrap();
        fs::write(src.join("real-dir/in.txt"), "in").unwrap();
        fs::write(src.join("real.txt"), "real").unwrap();
        symlink("real.txt", src.join("to-file")).unwrap();
        symlink("real-dir", src.join("to-dir")).unwrap();
        symlink("nowhere", src.join("to-nothing")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("to-dir"), "OLD file named to-dir").unwrap();
        let archive = dest.join("pack.tar");
        pack("tar", &src, &["real-dir", "real.txt", "to-dir", "to-file", "to-nothing"], &archive);

        let done = extract_archive_here(text(&archive), text(&dest)).unwrap();

        assert_eq!(
            done,
            ExtractHereOutcome {
                published: names(&["real-dir", "real.txt", "to-file", "to-nothing"]),
                skipped: vec![skip("to-dir", false)],
                ..Default::default()
            }
        );
        assert_eq!(
            tree(&dest),
            sorted(vec![
                sized(&archive),
                "real-dir/".to_string(),
                "real-dir/in.txt = in".to_string(),
                "real.txt = real".to_string(),
                "to-dir = OLD file named to-dir".to_string(),
                "to-file -> real.txt".to_string(),
                "to-nothing -> nowhere".to_string(),
            ])
        );
    }

    /// A type no tool is listed for is refused before anything is created.
    #[test]
    fn an_unsupported_type_creates_nothing_d230() {
        let root = owned("unsupported");
        let dest = root.0.join("dest");
        fs::create_dir(&dest).unwrap();
        let file = dest.join("plain.txt");
        fs::write(&file, "not an archive").unwrap();
        let run = |cmd: &str, _: &[&str]| -> Result<(), String> { panic!("{cmd} must not be started") };

        let result = extract_here_with(text(&file), text(&dest), &run, &rename_no_replace_io);

        assert_eq!(result, Err("unsupported archive type".to_string()));
        assert_eq!(tree(&dest), ["plain.txt = not an archive"]);
    }

    /// The message: plain only for a clean result; otherwise the counts, every
    /// skipped name with whole folders marked, each failure and anything left behind.
    #[test]
    fn the_message_counts_and_names_everything_d230() {
        let clean = ExtractHereOutcome { published: names(&["a", "b"]), ..Default::default() };
        assert_eq!(clean.summary(), "Extracted archive");
        assert_eq!(ExtractHereOutcome::default().summary(), "Nothing to extract: the archive is empty");

        let one = ExtractHereOutcome {
            published: names(&["a"]),
            skipped: vec![skip("note.txt", false)],
            ..Default::default()
        };
        assert_eq!(one.summary(), "Extracted 1 item. Skipped 1 that already exists: note.txt");
        let all = ExtractHereOutcome { skipped: vec![skip("note.txt", false), skip("photos", true)], ..Default::default() };
        assert_eq!(all.summary(), "Nothing extracted. Skipped 2 that already exist: note.txt, photos (whole folder)");
        let folder = ExtractHereOutcome {
            published: names(&["a", "b", "c"]),
            skipped: vec![skip("photos", true)],
            ..Default::default()
        };
        assert_eq!(folder.summary(), "Extracted 3 items. Skipped 1 that already exists: photos (whole folder)");
        let failed = ExtractHereOutcome { failed: vec![("f".to_string(), "why".to_string())], ..Default::default() };
        assert_eq!(failed.summary(), "Nothing extracted. Failed on 1: f (why)");
        let left = ExtractHereOutcome {
            published: names(&["a"]),
            leftover: Some(("/p/.wf-extract-1-2".to_string(), "why".to_string())),
            ..Default::default()
        };
        assert_eq!(left.summary(), "Extracted 1 item. Temporary folder not removed: /p/.wf-extract-1-2 (why)");
        let mixed = ExtractHereOutcome {
            published: names(&["a"]),
            skipped: vec![skip("s", false)],
            failed: vec![("f1".to_string(), "why 1".to_string()), ("f2".to_string(), "why 2".to_string())],
            leftover: Some(("/p/.wf-extract-1-2".to_string(), "why 3".to_string())),
        };
        assert_eq!(
            mixed.summary(),
            "Extracted 1 item. Skipped 1 that already exists: s. Failed on 2: f1 (why 1), f2 (why 2). \
             Temporary folder not removed: /p/.wf-extract-1-2 (why 3)"
        );
        for result in [&one, &all, &folder, &failed, &left, &mixed] {
            assert_ne!(result.summary(), "Extracted archive", "{result:?}");
        }
    }
}

/// D-231: the exact Trash batch on a fixture Trash of each test's own. The moves
/// are real renames into that Trash with real `.trashinfo` files. The last test
/// uses the real GIO call instead, in a home folder of its own. The user's Trash
/// is never read or changed.
#[cfg(test)]
mod partial_trash_tests_d231 {
    use super::extract_here_tests_d230::{tree, Owned};
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// What one batch reported: every moved item with its receipt and guard,
    /// the item it stopped at (its state, its path, the reason), and the items
    /// it never tried.
    #[derive(Debug)]
    struct Seen {
        done: Vec<(TrashReceipt, Fingerprint)>,
        stop: Option<(&'static str, String, String)>,
        left: Vec<String>,
    }

    /// One exact Trash batch as a caller sees it. Its own function so the same
    /// test text also builds on a tree from before the repair, where the batch
    /// was all or an error.
    fn batch_d231(paths: &[String]) -> Seen {
        let batch = trash_paths_exact(paths);
        Seen {
            done: batch.done.into_iter().map(|item| (item.receipt, item.fingerprint)).collect(),
            stop: batch.stopped.map(|stop| match stop {
                TrashStop::NotMoved { path, reason } => ("not moved", path, reason),
                TrashStop::MovedUnidentified { path, reason } => ("moved, not identified", path, reason),
            }),
            left: batch.not_attempted,
        }
    }

    /// A folder of this test's own with a fixture Trash (`bin`) and a work
    /// folder holding `names`, each file with its own bytes.
    struct Case {
        /// First, so it is dropped first: `bin/files` is listable again before
        /// the folder is removed.
        trash: fixture_trash::Installed,
        _root: Owned,
        bin: PathBuf,
        work: PathBuf,
    }

    impl Case {
        fn path(&self, name: &str) -> String {
            self.work.join(name).to_string_lossy().to_string()
        }

        /// The receipt of the item `name` that sits in the Trash under `stem`.
        fn receipt(&self, name: &str, stem: &str) -> TrashReceipt {
            TrashReceipt { original: self.path(name), selector: self.bin.join("files").join(stem).to_string_lossy().to_string() }
        }

        fn in_trash(&self) -> Vec<String> {
            tree(&self.bin.join("files"))
        }
    }

    /// A new folder of this test's own under the scratch root.
    fn own_root(tag: &str) -> Owned {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("wf-d231-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        // Another test's cleanup may remove the shared root while it is empty.
        (0..50)
            .find_map(|_| {
                let dir = scratch_root().join(&name);
                match fs::create_dir(&dir) {
                    Ok(()) => Some(Owned(dir)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => panic!("a new folder of this test's own, {}: {e}", dir.display()),
                }
            })
            .expect("the scratch root kept disappearing")
    }

    fn case(tag: &str, names: &[&str]) -> Case {
        let root = own_root(tag);
        let (bin, work) = (root.0.join("bin"), root.0.join("work"));
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&work).unwrap();
        for name in names {
            fs::write(work.join(name), format!("{name} bytes")).unwrap();
        }
        let trash = fixture_trash::install(&bin);
        Case { trash, _root: root, bin, work }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        names
    }

    fn denied() -> String {
        std::io::Error::from_raw_os_error(libc::EACCES).to_string()
    }

    const NOTHING: [&str; 0] = [];

    #[test]
    fn a_later_refusal_keeps_the_moved_item_d231() {
        let case = case("refuse", &["a.txt", "b.txt", "c.txt"]);
        case.trash.refuse("b.txt");
        let guard_a = fingerprint(&case.path("a.txt")).unwrap();
        let seen = batch_d231(&[case.path("a.txt"), case.path("b.txt"), case.path("c.txt")]);

        // Disk: the first really moved, the refused one and the third are as they were.
        assert_eq!(tree(&case.work), ["b.txt = b.txt bytes", "c.txt = c.txt bytes"]);
        assert_eq!(case.in_trash(), ["a.txt = a.txt bytes"]);
        assert_eq!(case.trash.asked(), [case.path("a.txt"), case.path("b.txt")], "the batch stops at the first failure");
        // The record: the moved item keeps its exact receipt and its own guard.
        let a = case.receipt("a.txt", "a.txt");
        assert_eq!(seen.done, [(a.clone(), guard_a)], "the item already in Trash lost its record: {seen:?}");
        assert_eq!(seen.stop, Some(("not moved", case.path("b.txt"), "the fixture Trash refused this item".to_string())));
        assert_eq!(seen.left, [case.path("c.txt")]);
        // And that record restores the exact payload.
        restore_from_trash_exact(&[a]).unwrap();
        assert_eq!(tree(&case.work), ["a.txt = a.txt bytes", "b.txt = b.txt bytes", "c.txt = c.txt bytes"]);
        assert_eq!(case.in_trash(), NOTHING);
        assert_eq!(tree(&case.bin.join("info")), NOTHING);
    }

    #[test]
    fn a_whole_batch_pairs_every_item_with_its_own_guard_d231() {
        let case = case("whole", &[]);
        fs::write(case.work.join("a.txt"), "first a").unwrap();
        fs::create_dir(case.work.join("sub")).unwrap();
        fs::write(case.work.join("sub/a.txt"), "second a").unwrap();
        fs::create_dir(case.work.join("folder")).unwrap();
        fs::write(case.work.join("folder/inner.txt"), "inner").unwrap();
        let paths = [case.path("a.txt"), case.path("sub/a.txt"), case.path("folder")];
        let guards: Vec<Fingerprint> = paths.iter().map(|path| fingerprint(path).unwrap()).collect();
        let before = tree(&case.work);
        assert!(!case.bin.join("files").exists(), "this Trash has never been used");

        let seen = batch_d231(&paths);

        assert_eq!(seen.stop, None);
        assert_eq!(seen.left, NOTHING);
        assert_eq!(tree(&case.work), ["sub/"]);
        assert_eq!(case.in_trash(), ["a.txt = first a", "a.txt.2 = second a", "folder/", "folder/inner.txt = inner"]);
        assert_eq!(case.trash.asked(), paths);
        // Each item with the entry it really went to, and the guard taken from that item.
        let receipts = [case.receipt("a.txt", "a.txt"), case.receipt("sub/a.txt", "a.txt.2"), case.receipt("folder", "folder")];
        let expected: Vec<(TrashReceipt, Fingerprint)> = receipts.iter().cloned().zip(guards).collect();
        assert_eq!(seen.done, expected);
        for (receipt, guard) in &seen.done {
            assert_eq!(&fingerprint(&receipt.selector).unwrap(), guard, "the guard is not this payload's: {receipt:?}");
        }
        restore_from_trash_exact(&receipts).unwrap();
        assert_eq!(tree(&case.work), before);
        assert_eq!(case.in_trash(), NOTHING);
    }

    #[test]
    fn an_unreadable_later_item_is_left_where_it_is_d231() {
        let case = case("unreadable", &["a.txt", "b.txt", "c.txt"]);
        let b = case.work.join("b.txt");
        fs::set_permissions(&b, fs::Permissions::from_mode(0o000)).unwrap();
        let guard_a = fingerprint(&case.path("a.txt")).unwrap();
        let seen = batch_d231(&[case.path("a.txt"), case.path("b.txt"), case.path("c.txt")]);

        // No guard can be taken from it, so it is not moved and nothing after it is tried.
        assert_eq!(case.trash.asked(), [case.path("a.txt")], "an item with no guard was moved");
        assert_eq!(names(&case.work), ["b.txt", "c.txt"]);
        assert_eq!(fs::symlink_metadata(&b).unwrap().permissions().mode() & 0o777, 0o000);
        assert_eq!(case.in_trash(), ["a.txt = a.txt bytes"]);
        let a = case.receipt("a.txt", "a.txt");
        assert_eq!(seen.done, [(a.clone(), guard_a)], "the item already in Trash lost its record: {seen:?}");
        assert_eq!(seen.stop, Some(("not moved", case.path("b.txt"), denied())));
        assert_eq!(seen.left, [case.path("c.txt")]);
        fs::set_permissions(&b, fs::Permissions::from_mode(0o600)).unwrap();
        restore_from_trash_exact(&[a]).unwrap();
        assert_eq!(tree(&case.work), ["a.txt = a.txt bytes", "b.txt = b.txt bytes", "c.txt = c.txt bytes"]);
    }

    #[test]
    fn a_second_new_entry_keeps_the_earlier_receipt_d231() {
        let case = case("twin", &["a.txt", "b.txt", "c.txt"]);
        case.trash.twin("b.txt");
        let guard_a = fingerprint(&case.path("a.txt")).unwrap();
        let seen = batch_d231(&[case.path("a.txt"), case.path("b.txt"), case.path("c.txt")]);

        assert_eq!(tree(&case.work), ["c.txt = c.txt bytes"]);
        assert_eq!(case.in_trash(), ["a.txt = a.txt bytes", "b.txt = b.txt bytes", "b.txt.twin = someone else's item"]);
        assert_eq!(case.trash.asked(), [case.path("a.txt"), case.path("b.txt")]);
        let a = case.receipt("a.txt", "a.txt");
        assert_eq!(seen.done, [(a.clone(), guard_a)], "the item already in Trash lost its record: {seen:?}");
        // The second item is in Trash, and no entry is guessed to be its own.
        assert_eq!(seen.stop, Some(("moved, not identified", case.path("b.txt"), "2 new Trash entries, expected 1".to_string())));
        assert_eq!(seen.left, [case.path("c.txt")]);
        restore_from_trash_exact(&[a]).unwrap();
        assert_eq!(tree(&case.work), ["a.txt = a.txt bytes", "c.txt = c.txt bytes"]);
        assert_eq!(case.in_trash(), ["b.txt = b.txt bytes", "b.txt.twin = someone else's item"]);
    }

    #[test]
    fn a_trash_that_cannot_be_listed_after_a_move_keeps_the_earlier_receipt_d231() {
        let case = case("blind-after", &["a.txt", "b.txt", "c.txt"]);
        case.trash.blind_after("b.txt");
        let guard_a = fingerprint(&case.path("a.txt")).unwrap();
        let seen = batch_d231(&[case.path("a.txt"), case.path("b.txt"), case.path("c.txt")]);
        case.trash.blind(false);

        assert_eq!(tree(&case.work), ["c.txt = c.txt bytes"]);
        assert_eq!(case.in_trash(), ["a.txt = a.txt bytes", "b.txt = b.txt bytes"]);
        assert_eq!(case.trash.asked(), [case.path("a.txt"), case.path("b.txt")]);
        let a = case.receipt("a.txt", "a.txt");
        assert_eq!(seen.done, [(a.clone(), guard_a)], "the item already in Trash lost its record: {seen:?}");
        assert_eq!(
            seen.stop,
            Some(("moved, not identified", case.path("b.txt"), format!("cannot read the Trash folder: {}", denied())))
        );
        assert_eq!(seen.left, [case.path("c.txt")]);
        restore_from_trash_exact(&[a]).unwrap();
        assert_eq!(tree(&case.work), ["a.txt = a.txt bytes", "c.txt = c.txt bytes"]);
    }

    #[test]
    fn a_trash_that_cannot_be_listed_gets_nothing_moved_into_it_d231() {
        let case = case("blind", &["a.txt", "b.txt"]);
        fs::create_dir(case.bin.join("files")).unwrap();
        case.trash.blind(true);
        let seen = batch_d231(&[case.path("a.txt"), case.path("b.txt")]);
        case.trash.blind(false);

        // An unreadable Trash folder is an error, not an empty one.
        assert_eq!(case.trash.asked(), NOTHING, "an item was moved into a Trash that could not be read");
        assert_eq!(tree(&case.work), ["a.txt = a.txt bytes", "b.txt = b.txt bytes"]);
        assert_eq!(case.in_trash(), NOTHING);
        assert!(seen.done.is_empty(), "{seen:?}");
        assert_eq!(
            seen.stop,
            Some(("not moved", case.path("a.txt"), format!("cannot read the Trash folder: {}", denied())))
        );
        assert_eq!(seen.left, [case.path("b.txt")]);
    }

    /// The same batch on the real GIO Trash call, with no fixture. GIO takes
    /// its Trash from the home folder its process started with, so this starts
    /// a second copy of the test program with HOME set to a folder of this
    /// test's own: the only Trash that copy works in is inside this test's
    /// root. The later refusal is real as well: GIO cannot move a file out of
    /// a folder that does not allow changes.
    #[test]
    fn a_real_gio_refusal_keeps_the_moved_item_d231() {
        const ROOT: &str = "WF_D231_GIO_CHILD_ROOT";
        if let Some(root) = std::env::var_os(ROOT) {
            return real_gio_refusal(Path::new(&root));
        }
        let root = own_root("gio");
        let home = root.0.join("home");
        fs::create_dir(&home).unwrap();
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["a_real_gio_refusal_keeps_the_moved_item_d231", "--nocapture"])
            .env("HOME", &home)
            .env(ROOT, &root.0)
            .env_remove("XDG_DATA_HOME")
            .output()
            .expect("start the second copy of the test program");
        let (said, noted) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && said.contains("1 passed"), "the copy with its own home failed:\n{said}\n{noted}");
        eprint!("{noted}");
    }

    /// Runs in the second copy, where HOME is `root/home`.
    fn real_gio_refusal(root: &Path) {
        let home = root.join("home");
        // Never on the real home: only on a root this test made.
        assert!(root.starts_with(scratch_root()) && root != scratch_root(), "{} is not this test's own", root.display());
        assert_eq!(std::env::var_os("HOME"), Some(home.clone().into_os_string()));
        assert_eq!(std::env::var_os("XDG_DATA_HOME"), None);
        assert_eq!(fixture_trash::dir(), None, "this copy has no fixture Trash");
        let (work, locked) = (root.join("work"), root.join("work/locked"));
        fs::create_dir_all(&locked).unwrap();
        let path = |name: &str| work.join(name).to_string_lossy().to_string();
        for name in ["a.txt", "locked/b.txt", "c.txt"] {
            fs::write(work.join(name), format!("{name} bytes")).unwrap();
        }
        let guard_a = fingerprint(&path("a.txt")).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let seen = batch_d231(&[path("a.txt"), path("locked/b.txt"), path("c.txt")]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("real GIO batch: {seen:?}");

        // GIO put a.txt, and nothing else, in the Trash of this home with its record.
        let trash = home.join(".local/share/Trash");
        assert_eq!(tree(&trash.join("files")), ["a.txt = a.txt bytes"]);
        assert_eq!(names(&trash.join("info")), ["a.txt.trashinfo"]);
        let info = fs::read_to_string(trash.join("info/a.txt.trashinfo")).unwrap();
        assert!(info.contains(&format!("Path={}\n", path("a.txt"))), "{info}");
        assert_eq!(tree(&work), ["c.txt = c.txt bytes", "locked/", "locked/b.txt = locked/b.txt bytes"]);
        // The moved item keeps its exact receipt and guard although b.txt was refused.
        let a = TrashReceipt { original: path("a.txt"), selector: trash.join("files/a.txt").to_string_lossy().to_string() };
        assert_eq!(seen.done, [(a.clone(), guard_a)], "the moved item's receipt was lost");
        let (state, at, reason) = seen.stop.clone().expect("the batch stopped");
        assert_eq!((state, at), ("not moved", path("locked/b.txt")));
        assert!(reason.contains("Permission denied"), "{reason}");
        assert_eq!(seen.left, [path("c.txt")]);
        // The exact restore takes it back out of this home's Trash.
        restore_from_trash_exact(&[a]).unwrap();
        assert_eq!(
            tree(&work),
            ["a.txt = a.txt bytes", "c.txt = c.txt bytes", "locked/", "locked/b.txt = locked/b.txt bytes"]
        );
        assert_eq!(names(&trash.join("files")), NOTHING);
        assert_eq!(names(&trash.join("info")), NOTHING);
    }
}

/// D-232: the clipboard formats on their own. No clipboard and no display is
/// used here; the bytes are the ones the three forms carry.
#[cfg(test)]
mod clip_format_tests_d232 {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn files(intent: ClipIntent, paths: &[&str]) -> Result<ClipFiles, String> {
        Ok(ClipFiles { intent, paths: paths.iter().map(PathBuf::from).collect() })
    }
    fn gnome(data: &str) -> Result<ClipFiles, String> {
        clip_decode(&ClipRead::Gnome { gnome: data.as_bytes().to_vec(), kde_cut: None })
    }
    fn gnome_kde(data: &str, marker: &[u8]) -> Result<ClipFiles, String> {
        clip_decode(&ClipRead::Gnome { gnome: data.as_bytes().to_vec(), kde_cut: Some(marker.to_vec()) })
    }
    fn kde(before: &[u8], uris: &str, after: &[u8]) -> Result<ClipFiles, String> {
        clip_decode(&ClipRead::Kde {
            cut_before: before.to_vec(),
            uris: uris.as_bytes().to_vec(),
            cut_after: after.to_vec(),
        })
    }
    fn uri_only(uris: &str) -> Result<ClipFiles, String> {
        clip_decode(&ClipRead::UriOnly { uris: uris.as_bytes().to_vec() })
    }
    fn refused(result: Result<ClipFiles, String>) -> String {
        result.expect_err("this clipboard content is refused")
    }

    const PLAIN: &str = "/tmp/wf d232/a b.txt";
    const ODD: &str = "/tmp/wf d232/100%#é\nx";
    const PLAIN_URI: &str = "file:///tmp/wf%20d232/a%20b.txt";
    const ODD_URI: &str = "file:///tmp/wf%20d232/100%25%23%C3%A9%0Ax";

    #[test]
    fn encode_writes_the_three_forms_exactly_d232() {
        let paths = [PathBuf::from(PLAIN), PathBuf::from(ODD)];
        let cut = clip_encode(&paths, ClipIntent::Cut).unwrap();
        assert_eq!(String::from_utf8(cut.gnome).unwrap(), format!("cut\n{PLAIN_URI}\n{ODD_URI}"));
        assert_eq!(String::from_utf8(cut.uri_list).unwrap(), format!("{PLAIN_URI}\r\n{ODD_URI}\r\n"));
        assert_eq!(cut.kde_cut, b"1");
        let copy = clip_encode(&paths[..1], ClipIntent::Copy).unwrap();
        assert_eq!(String::from_utf8(copy.gnome).unwrap(), format!("copy\n{PLAIN_URI}"));
        assert_eq!(String::from_utf8(copy.uri_list).unwrap(), format!("{PLAIN_URI}\r\n"));
        assert_eq!(copy.kde_cut, b"0");
        // Nothing, or a path that is not full, is refused instead of guessed.
        assert_eq!(clip_encode(&[], ClipIntent::Copy).unwrap_err(), "there is nothing to put on the clipboard");
        assert_eq!(clip_encode(&[PathBuf::from("a.txt")], ClipIntent::Copy).unwrap_err(), "a.txt is not a full path");
    }

    #[test]
    fn every_form_reads_back_the_same_files_d232() {
        // A name that is not UTF-8 goes through as its exact bytes.
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/wf d232/\xff\xfe.bin".to_vec()));
        let paths = vec![PathBuf::from(PLAIN), PathBuf::from(ODD), raw];
        for intent in [ClipIntent::Copy, ClipIntent::Cut] {
            let want = Ok(ClipFiles { intent, paths: paths.clone() });
            let payload = clip_encode(&paths, intent).unwrap();
            let both = ClipRead::Gnome { gnome: payload.gnome.clone(), kde_cut: Some(payload.kde_cut.clone()) };
            assert_eq!(clip_decode(&both), want, "the GNOME form with the KDE marker");
            let alone = ClipRead::Gnome { gnome: payload.gnome.clone(), kde_cut: None };
            assert_eq!(clip_decode(&alone), want, "the GNOME form alone");
            let marked = ClipRead::Kde {
                cut_before: payload.kde_cut.clone(),
                uris: payload.uri_list.clone(),
                cut_after: payload.kde_cut.clone(),
            };
            assert_eq!(clip_decode(&marked), want, "the URI list with the KDE marker");
            // A URI list alone never moves anything.
            let list = ClipRead::UriOnly { uris: payload.uri_list.clone() };
            assert_eq!(clip_decode(&list), Ok(ClipFiles { intent: ClipIntent::Copy, paths: paths.clone() }));
        }
    }

    #[test]
    fn the_gnome_form_needs_copy_or_cut_d232() {
        assert_eq!(gnome("copy\nfile:///tmp/a"), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(gnome("cut\nfile:///tmp/a\nfile:///tmp/b\n"), files(ClipIntent::Cut, &["/tmp/a", "/tmp/b"]));
        assert_eq!(gnome("cut\r\nfile:///tmp/a\r\n\0"), files(ClipIntent::Cut, &["/tmp/a"]), "line ends and a final zero byte");
        assert_eq!(gnome("copy\nfile:///tmp/a\nfile:///tmp/a"), files(ClipIntent::Copy, &["/tmp/a"]), "the same file twice");
        for first in ["", "move", "Cut", "copy ", " cut", "file:///tmp/a", "cutfile:///tmp/a"] {
            assert_eq!(
                refused(gnome(&format!("{first}\nfile:///tmp/a"))),
                "the clipboard does not say copy or cut",
                "first line {first:?}"
            );
        }
        assert_eq!(refused(gnome("cut")), "the clipboard holds no files");
        assert_eq!(refused(gnome("copy\n\n\n")), "the clipboard holds no files");
        // A `#` line is a note only in a URI list; here it is a bad entry.
        assert_eq!(refused(gnome("copy\n#file:///tmp/a")), "#file:///tmp/a is not a valid file address");
        assert_eq!(
            refused(clip_decode(&ClipRead::Gnome { gnome: b"cut\nfile:///tmp/\xff".to_vec(), kde_cut: None })),
            "the clipboard data is not text"
        );
    }

    #[test]
    fn a_marker_is_never_guessed_d232() {
        // Both markers present: they must agree.
        assert_eq!(gnome_kde("cut\nfile:///tmp/a", b"1"), files(ClipIntent::Cut, &["/tmp/a"]));
        assert_eq!(gnome_kde("copy\nfile:///tmp/a", b"0\0"), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(refused(gnome_kde("copy\nfile:///tmp/a", b"1")), "the clipboard's copy and cut markers disagree");
        assert_eq!(refused(gnome_kde("cut\nfile:///tmp/a", b"0")), "the clipboard's copy and cut markers disagree");
        for marker in [&b""[..], b"2", b"11", b"cut", b"true", b"\xff", b"1 0"] {
            assert_eq!(
                refused(gnome_kde("cut\nfile:///tmp/a", marker)),
                "the clipboard's cut marker is not 0 or 1",
                "marker {marker:?} beside the GNOME form"
            );
            assert_eq!(
                refused(kde(marker, "file:///tmp/a\r\n", marker)),
                "the clipboard's cut marker is not 0 or 1",
                "marker {marker:?} beside a URI list"
            );
        }
        // The KDE form: the marker read before the list and after it must match.
        assert_eq!(kde(b"1", "file:///tmp/a\r\n", b"1"), files(ClipIntent::Cut, &["/tmp/a"]));
        assert_eq!(kde(b"0", "file:///tmp/a\r\n", b"0"), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(refused(kde(b"1", "file:///tmp/a\r\n", b"0")), "the clipboard changed while it was read");
        assert_eq!(refused(kde(b"0", "file:///tmp/a\r\n", b"1")), "the clipboard changed while it was read");
    }

    #[test]
    fn a_uri_list_alone_is_a_copy_d232() {
        assert_eq!(
            uri_only("# a note\r\nfile:///tmp/a\r\n\r\n  file:///tmp/b%20c  \r\n"),
            files(ClipIntent::Copy, &["/tmp/a", "/tmp/b c"])
        );
        assert_eq!(uri_only("file:///tmp/a"), files(ClipIntent::Copy, &["/tmp/a"]), "no final line end");
        assert_eq!(uri_only("file://localhost/tmp/a\n"), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(uri_only(&format!("file://{}/tmp/a\n", glib::host_name())), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(uri_only("FILE:///tmp/a\n"), files(ClipIntent::Copy, &["/tmp/a"]));
        assert_eq!(uri_only(&format!("{PLAIN_URI}\r\n{ODD_URI}\r\n")), files(ClipIntent::Copy, &[PLAIN, ODD]));
        for empty in ["", "\r\n", "# only a note\r\n", "\0\0"] {
            assert_eq!(refused(uri_only(empty)), "the clipboard holds no files", "{empty:?}");
        }
    }

    #[test]
    fn one_bad_entry_refuses_the_whole_paste_d232() {
        let cases = [
            ("sftp://host/tmp/a", "sftp://host/tmp/a is not a local file (sftp is not supported)"),
            ("smb://server/share/a", "smb://server/share/a is not a local file (smb is not supported)"),
            ("trash:///a.txt", "trash:///a.txt is not a local file (trash is not supported)"),
            ("https://example.org/a", "https://example.org/a is not a local file (https is not supported)"),
            ("file://elsewhere.invalid/tmp/a", "file://elsewhere.invalid/tmp/a is on another computer"),
            ("/tmp/a", "/tmp/a is not a file address"),
            ("a.txt", "a.txt is not a file address"),
            ("file:tmp/a", "file:tmp/a is not a valid file address"),
            ("file:///tmp/a%00b", "file:///tmp/a%00b is not a valid file address"),
            ("file:///tmp/a%2Fb", "file:///tmp/a%2Fb is not a valid file address"),
            ("file:///tmp/a%zz", "file:///tmp/a%zz is not a valid file address"),
            ("file:///tmp/a#b", "file:///tmp/a#b is not a valid file address"),
            ("file:///tmp/a?b", "file:///tmp/a?b is not a valid file address"),
            ("file:///", "file:/// does not name a file or folder"),
            ("file:///tmp/..", "file:///tmp/.. does not name a file or folder"),
        ];
        for (bad, why) in cases {
            assert_eq!(refused(uri_only(&format!("{bad}\r\n"))), why, "alone");
            // A good entry before it and after it does not get through either.
            let mixed = format!("file:///tmp/good-1\n{bad}\nfile:///tmp/good-2");
            assert_eq!(refused(uri_only(&mixed)), why, "between two good entries, URI list");
            assert_eq!(refused(gnome(&format!("cut\n{mixed}"))), why, "between two good entries, GNOME form");
            assert_eq!(refused(kde(b"1", &mixed, b"1")), why, "between two good entries, KDE form");
        }
        // A long entry is shown cut to its first 80 characters.
        let long = format!("ftp://host/{}", "x".repeat(200));
        assert_eq!(
            refused(uri_only(&long)),
            format!("{} is not a local file (ftp is not supported)", &long[..80])
        );
        // A control character in an entry never reaches the status line.
        assert_eq!(refused(uri_only("file:///tmp/a\0b\r\n")), "file:///tmp/a\u{FFFD}b is not a valid file address");
        assert_eq!(refused(gnome("copy\nbell\u{7}here")), "bell\u{FFFD}here is not a file address");
    }
}

/// Vault content must never be listed anywhere Wayfinder shows things again:
/// the Recent files strip and the Recent folders list both read from disk
/// history, and a vault's whole point is that its contents are not advertised
/// on a Home page somebody else can see over your shoulder.
///
/// A path counts as vault content when any folder in it is named for a vault,
/// matched on the lowercased name containing "vault". That is deliberately
/// wide: `Vaults`, `Obsidian Vault` and `Field-Notes-Vault-notes` are all
/// caught, and the cost of catching one folder too many is a file missing
/// from a convenience list, while the cost of missing one is private content
/// on screen.
pub fn is_vault_content(path: &str) -> bool {
    Path::new(path).components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .contains("vault")
    })
}

#[cfg(test)]
mod vault_privacy_tests {
    use super::is_vault_content;

    /// D-247: what the Recent lists must refuse, and what they must not.
    #[test]
    fn vault_content_is_recognised_wherever_it_sits_d247() {
        for private in [
            "/home/user/Vaults/Daily Logs/one.md",
            "/home/user/References/Vaults/two.md",
            "/home/user/Obsidian/Field Notes Vault/note.md",
            "/home/user/Documents/Obsidian Vault/note.md",
            "/home/user/References/Field-Notes-Vault-notes/note.md",
            "/home/user/vaults/lower.md",
        ] {
            assert!(is_vault_content(private), "{private} must never be listed");
        }
        for ordinary in [
            "/home/user/Documents/report.odt",
            "/home/user/Projects/Open Projects/wayfinder/README.md",
            "/home/user/Pictures/holiday.jpg",
        ] {
            assert!(!is_vault_content(ordinary), "{ordinary} is ordinary");
        }
    }
}

/// D-251: gdk-pixbuf on this machine has no AVIF, HEIC or JPEG-XL loader, so a
/// thumbnail for one of those files decoded to nothing and the tile kept its
/// plain file icon forever. These are the extensions that have to be decoded
/// by an outside tool first and thumbnailed from the result.
const TRANSCODE_THUMB_EXT: [&str; 5] = ["avif", "heic", "heif", "avifs", "jxl"];

pub fn needs_transcoded_thumbnail(path: &str) -> bool {
    has_extension(path, &TRANSCODE_THUMB_EXT)
}

/// D-260: Details, List and Compact can draw a thumbnail in the same slot the
/// row already reserves for its file-type icon, behind a setting. The rules
/// that decide whether a row earns a decode and how big that decode is need no
/// GTK, so they live here and are unit-tested without a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListThumbnailMetrics {
    /// Edge of the square the picture is drawn in, in logical pixels.
    pub display: i32,
    /// Edge handed to the decoder. Twice `display`, so the scaled decode stays
    /// sharp without paying to decode the full-size picture.
    pub decode: i32,
}

/// The icon each list view already draws, from `file_icon` in native.rs:
/// Details reserves a 32x28 slot with a 28x24 icon, Compact a 20x18 slot with
/// a 16x16 icon, and both use the shorter edge as the exact pixel size. The
/// thumbnail must match that shorter edge or the row height would change.
pub fn list_thumbnail_metrics(icon_zoom: f64, compact: bool) -> ListThumbnailMetrics {
    let icon_base = if compact { 16.0 } else { 24.0 };
    let display = (icon_base * icon_zoom).round().max(1.0) as i32;
    ListThumbnailMetrics {
        display,
        decode: (display * 2).max(1),
    }
}

/// A row earns a thumbnail only when a real image file is behind it. Folders
/// have no picture, and a ZIP member has no filesystem path to decode from
/// (D-188), so both keep the ordinary file-type icon. Video rows are excluded
/// here as well: this setting covers still images only, while Grid's own video
/// poster queue is unchanged.
pub fn wants_image_thumbnail(entry: &Entry) -> bool {
    entry.is_image
        && !entry.is_dir
        && !entry.source.is_zip()
        && entry.source.real_path().is_some()
}

#[cfg(test)]
mod list_thumbnail_tests {
    use super::{
        is_image_path, list_thumbnail_metrics, needs_transcoded_thumbnail, wants_image_thumbnail,
        Entry, EntrySource, ListThumbnailMetrics,
    };

    fn row(name: &str, is_image: bool, is_dir: bool, source: EntrySource) -> Entry {
        Entry {
            name: name.to_string(),
            source,
            is_dir,
            size: 0,
            modified: 0,
            is_image,
            icon: String::new(),
            metadata_error: None,
        }
    }

    #[test]
    fn thumbnail_metrics_match_the_icon_each_list_view_draws_d260() {
        assert_eq!(
            list_thumbnail_metrics(1.0, false),
            ListThumbnailMetrics { display: 24, decode: 48 },
            "Details draws a 28x24 icon, so the shorter edge is 24"
        );
        assert_eq!(
            list_thumbnail_metrics(1.0, true),
            ListThumbnailMetrics { display: 16, decode: 32 },
            "Compact draws a 16x16 icon"
        );
        assert_eq!(list_thumbnail_metrics(1.5, false), ListThumbnailMetrics { display: 36, decode: 72 });
        assert_eq!(list_thumbnail_metrics(2.0, true), ListThumbnailMetrics { display: 32, decode: 64 });
        // Zoom is clamped to 0.6..=2.4 in the app; the floor still holds.
        assert_eq!(list_thumbnail_metrics(0.1, true).display, 2);
    }

    #[test]
    fn only_real_still_images_earn_a_list_thumbnail_d260() {
        let png = row("a.png", true, false, EntrySource::real("/tmp/a.png"));
        assert!(wants_image_thumbnail(&png));

        // D-251: an outside-decoder image is still an image and is transcoded
        // by the same queue, so it must be eligible here.
        for name in ["one.avif", "two.heic", "three.heif", "four.avifs", "five.jxl"] {
            let path = format!("/tmp/{name}");
            assert!(is_image_path(&path) && needs_transcoded_thumbnail(&path));
            let e = row(name, true, false, EntrySource::real(path));
            assert!(wants_image_thumbnail(&e), "{name} must be queued like any image");
        }

        // Not an image.
        assert!(!wants_image_thumbnail(&row("a.mp4", false, false, EntrySource::real("/tmp/a.mp4"))));
        // A folder whose name happens to end in an image extension.
        assert!(!wants_image_thumbnail(&row("a.png", true, true, EntrySource::real("/tmp/a.png"))));
        // A ZIP member has no real path to decode from (D-188).
        let member = EntrySource::zip_member("/tmp/a.zip", "inside.png", 10, 0, 100, 0);
        assert!(!wants_image_thumbnail(&row("inside.png", true, false, member)));
    }
}

#[cfg(test)]
mod transcoded_thumbnail_tests {
    use super::{is_image_path, needs_transcoded_thumbnail};

    #[test]
    fn avif_and_heic_want_an_outside_decoder_but_png_does_not_d251() {
        for name in [
            "/home/user/Pictures/one.avif",
            "/home/user/Pictures/ONE.AVIF",
            "/home/user/Pictures/two.heic",
            "/home/user/Pictures/three.heif",
            "/home/user/Pictures/four.jxl",
        ] {
            assert!(needs_transcoded_thumbnail(name), "{name} needs decoding");
            assert!(is_image_path(name), "{name} is still an image");
        }
        for name in [
            "/home/user/Pictures/five.png",
            "/home/user/Pictures/six.jpg",
            "/home/user/Pictures/seven.webp",
            "/home/user/Videos/eight.mp4",
            "/home/user/Documents/nine",
        ] {
            assert!(!needs_transcoded_thumbnail(name), "{name} decodes in-process");
        }
    }
}

/// D-262: what the address bar does with whatever was typed or pasted into it.
///
/// the owner, 11 November 2026: he pasted `example-project/art/anim/comet/`
/// and nothing happened, then `example-project/art/anim/` and nothing
/// happened again. Both are real folders; neither is an absolute path. The bar
/// handed the text straight to the lister, which can only open an absolute
/// path, and a failure said nothing at all.
///
/// A path copied out of a terminal, an editor or a chat message arrives with
/// quotes around it, a `file://` on the front, `%20` for every space, a `~`
/// for the home folder, a trailing slash, or as a fragment relative to
/// something other than the folder on screen. All of that is answered here,
/// with no GTK, so each rule is a unit test rather than a thing to click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Dir,
    File,
    Missing,
}

/// Strips the decoration off a pasted path and expands `~`. The result is not
/// guaranteed to exist; `resolve_typed_path` decides that.
pub fn clean_typed_path(text: &str, home: &str) -> String {
    let mut s = text.trim().to_string();
    // A paste from a terminal or a chat often carries quotes.
    for (open, close) in [('"', '"'), ('\'', '\''), ('`', '`'), ('\u{201c}', '\u{201d}')] {
        if s.len() >= 2 && s.starts_with(open) && s.ends_with(close) {
            s = s[open.len_utf8()..s.len() - close.len_utf8()].trim().to_string();
        }
    }
    // A file manager, a browser or a GTK dialog hands out file:// URIs.
    if let Some(rest) = s.strip_prefix("file://") {
        s = percent_decode(rest.trim_start_matches("localhost"));
    } else if s.contains("%20") || s.contains("%2F") {
        s = percent_decode(&s);
    }
    // Shell escapes: `cd` output and tab completion escape spaces.
    if s.contains("\\ ") {
        s = s.replace("\\ ", " ");
    }
    if s == "~" {
        return home.to_string();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        s = format!("{}/{}", home.trim_end_matches('/'), rest);
    } else if let Some(rest) = s.strip_prefix("$HOME/") {
        s = format!("{}/{}", home.trim_end_matches('/'), rest);
    }
    // One trailing slash is how most people copy a folder; more than one, and
    // doubled separators inside, come from joining two halves by hand.
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    s
}

// `percent_decode` already lives above, next to the command-line parsing.

/// Where the address bar should go for `text`, or `None` when nothing matches.
///
/// `kind` answers whether a candidate is a folder, a file or missing, so this
/// is pure and testable. The order is deliberate and stops at the first hit:
///
/// 1. An absolute path, as given. A FILE lands in its folder, because pasting
///    a file's path means "show me this".
/// 2. The fragment under the folder on screen.
/// 3. The fragment under each ancestor of the folder on screen, nearest
///    first. This is what makes a repo-relative path work: standing anywhere
///    inside a project, `art/anim/comet` finds the project's own folder.
/// 4. The fragment under home.
/// 5. The fragment under each extra root, in the order given — the sidebar's
///    own folders, so a path copied out of a project reachable from the
///    sidebar still lands.
pub fn resolve_typed_path(
    text: &str,
    cwd: &str,
    home: &str,
    roots: &[String],
    kind: &dyn Fn(&str) -> PathKind,
) -> Option<String> {
    let cleaned = clean_typed_path(text, home);
    if cleaned.is_empty() {
        return None;
    }
    let settle = |candidate: String| -> Option<String> {
        match kind(&candidate) {
            PathKind::Dir => Some(candidate),
            PathKind::File => Path::new(&candidate)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned()),
            PathKind::Missing => None,
        }
    };
    if cleaned.starts_with('/') {
        return settle(cleaned);
    }
    let join = |base: &str| -> String {
        format!("{}/{}", base.trim_end_matches('/'), cleaned)
    };
    let mut bases: Vec<String> = Vec::new();
    if !cwd.is_empty() {
        bases.push(cwd.to_string());
        let mut here = Path::new(cwd);
        while let Some(parent) = here.parent() {
            let parent_str = parent.to_string_lossy().into_owned();
            if parent_str.is_empty() {
                break;
            }
            bases.push(parent_str);
            here = parent;
        }
    }
    if !home.is_empty() {
        bases.push(home.to_string());
    }
    bases.extend(roots.iter().cloned());
    let mut seen: Vec<String> = Vec::new();
    for base in bases {
        let base = base.trim_end_matches('/').to_string();
        if base.is_empty() || seen.contains(&base) {
            continue;
        }
        seen.push(base.clone());
        if let Some(found) = settle(join(&base)) {
            return Some(found);
        }
    }
    None
}

/// D-262, the last resort: a fragment that belongs to a tree somewhere else.
/// `example-project/art/anim/comet/` pasted while standing in home is
/// below no ancestor of home, so `resolve_typed_path` cannot see it. This
/// walks a bounded number of folders under the places that are already known
/// — the folder on screen, home, the sidebar's folders — widest-first, and
/// tries the fragment under each one. Hidden folders are skipped, and the
/// walk stops at `max_depth` levels or `max_dirs` folders, whichever comes
/// first, so a deep or enormous tree cannot stall the window.
pub fn search_for_fragment(
    fragment: &str,
    roots: &[String],
    max_depth: usize,
    max_dirs: usize,
    child_dirs: &dyn Fn(&str) -> Vec<String>,
    kind: &dyn Fn(&str) -> PathKind,
) -> Option<String> {
    if fragment.is_empty() || fragment.starts_with('/') {
        return None;
    }
    let settle = |candidate: String| -> Option<String> {
        match kind(&candidate) {
            PathKind::Dir => Some(candidate),
            PathKind::File => Path::new(&candidate)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned()),
            PathKind::Missing => None,
        }
    };
    let mut seen: Vec<String> = Vec::new();
    let mut level: Vec<String> = Vec::new();
    for root in roots {
        let root = root.trim_end_matches('/').to_string();
        if !root.is_empty() && !seen.contains(&root) {
            seen.push(root.clone());
            level.push(root);
        }
    }
    let mut visited = 0usize;
    for _ in 0..=max_depth {
        if level.is_empty() {
            return None;
        }
        let mut next: Vec<String> = Vec::new();
        for dir in &level {
            visited += 1;
            if visited > max_dirs {
                return None;
            }
            if let Some(found) = settle(format!("{dir}/{fragment}")) {
                return Some(found);
            }
            for child in child_dirs(dir) {
                if child.starts_with('.') {
                    continue;
                }
                let path = format!("{dir}/{child}");
                if !seen.contains(&path) {
                    seen.push(path.clone());
                    next.push(path);
                }
            }
        }
        level = next;
    }
    None
}

#[cfg(test)]
mod typed_path_tests {
    use super::{clean_typed_path, resolve_typed_path, search_for_fragment, PathKind};
    use std::path::Path;

    const HOME: &str = "/home/user";

    fn kind_from(dirs: &'static [&'static str], files: &'static [&'static str]) -> impl Fn(&str) -> PathKind {
        move |p: &str| {
            if dirs.contains(&p) {
                PathKind::Dir
            } else if files.contains(&p) {
                PathKind::File
            } else {
                PathKind::Missing
            }
        }
    }

    #[test]
    fn a_fragment_from_another_tree_is_found_by_a_bounded_walk_d262() {
        // What he actually pasted, while standing in home: the tree is four
        // folders down a path home knows nothing about.
        let dirs: &[&str] = &[
            "/home/user",
            "/home/user/Projects",
            "/home/user/Projects/Scratch",
            "/home/user/Projects/Scratch/audit",
            "/home/user/Projects/Scratch/audit/example-project",
            "/home/user/Projects/Scratch/audit/example-project/art",
            "/home/user/Projects/Scratch/audit/example-project/art/anim",
            "/home/user/Projects/Scratch/audit/example-project/art/anim/comet",
            "/home/user/.cache",
            "/home/user/.cache/huge",
        ];
        let children = |dir: &str| -> Vec<String> {
            dirs.iter()
                .filter(|d| Path::new(d).parent().map(|p| p.to_string_lossy().into_owned()).as_deref() == Some(dir))
                .filter_map(|d| Path::new(d).file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        };
        let kind = kind_from(dirs, &[]);
        let found = resolve_typed_path(
            "example-project/art/anim/comet/",
            HOME,
            HOME,
            &[HOME.to_string()],
            &kind,
        );
        assert_eq!(found, None, "the plain rules cannot see another tree");
        let found = search_for_fragment(
            "example-project/art/anim/comet",
            &[HOME.to_string()],
            5,
            4000,
            &children,
            &kind,
        );
        assert_eq!(
            found.as_deref(),
            Some("/home/user/Projects/Scratch/audit/example-project/art/anim/comet"),
            "the bounded walk must find the tree he pasted from"
        );
    }

    #[test]
    fn the_walk_gives_up_instead_of_stalling_d262() {
        let children = |dir: &str| -> Vec<String> {
            // An endlessly wide, endlessly deep tree.
            (0..50).map(|n| format!("{}-{n}", dir.len())).collect()
        };
        let kind = kind_from(&[], &[]);
        assert_eq!(
            search_for_fragment("nowhere/at/all", &[HOME.to_string()], 6, 500, &children, &kind),
            None,
            "a tree with no match must end on the caps, not run forever"
        );
    }

    #[test]
    fn decoration_comes_off_a_pasted_path_d262() {
        assert_eq!(clean_typed_path("  /tmp/a  ", HOME), "/tmp/a");
        assert_eq!(clean_typed_path("\"/tmp/a b\"", HOME), "/tmp/a b");
        assert_eq!(clean_typed_path("'/tmp/a'", HOME), "/tmp/a");
        assert_eq!(clean_typed_path("file:///tmp/a%20b", HOME), "/tmp/a b");
        assert_eq!(clean_typed_path("file://localhost/tmp/a", HOME), "/tmp/a");
        assert_eq!(clean_typed_path("/tmp/a\\ b", HOME), "/tmp/a b");
        assert_eq!(clean_typed_path("~", HOME), HOME);
        assert_eq!(clean_typed_path("~/Pictures/", HOME), "/home/user/Pictures");
        assert_eq!(clean_typed_path("$HOME/Pictures", HOME), "/home/user/Pictures");
        assert_eq!(clean_typed_path("/tmp//a///b//", HOME), "/tmp/a/b");
        // A relative fragment stays relative: resolving it is a separate job.
        assert_eq!(clean_typed_path("art/anim/comet/", HOME), "art/anim/comet");
    }

    #[test]
    fn an_absolute_path_is_taken_as_given_and_a_file_lands_in_its_folder_d262() {
        let kind = kind_from(&["/tmp/pics"], &["/tmp/pics/a.png"]);
        assert_eq!(
            resolve_typed_path("/tmp/pics/", "/home/user", HOME, &[], &kind).as_deref(),
            Some("/tmp/pics")
        );
        assert_eq!(
            resolve_typed_path("/tmp/pics/a.png", "/home/user", HOME, &[], &kind).as_deref(),
            Some("/tmp/pics")
        );
        assert_eq!(resolve_typed_path("/tmp/nope", "/home/user", HOME, &[], &kind), None);
    }

    /// The reported case: a project-relative fragment pasted while standing
    /// somewhere else inside the same project.
    #[test]
    fn a_project_relative_fragment_resolves_up_the_tree_d262() {
        let kind = kind_from(
            &["/home/user/Projects/demo/art/anim/comet", "/home/user/Projects/demo/art/anim"],
            &[],
        );
        let cwd = "/home/user/Projects/demo/art/sprites";
        assert_eq!(
            resolve_typed_path("art/anim/comet/", cwd, HOME, &[], &kind).as_deref(),
            Some("/home/user/Projects/demo/art/anim/comet")
        );
        assert_eq!(
            resolve_typed_path("art/anim/", cwd, HOME, &[], &kind).as_deref(),
            Some("/home/user/Projects/demo/art/anim")
        );
        // Nearest ancestor wins: standing in the folder itself, the plain name
        // of a child resolves to the child, not to a cousin higher up.
        let kind = kind_from(&["/home/user/Projects/demo/art/anim", "/home/user/anim"], &[]);
        assert_eq!(
            resolve_typed_path("anim", "/home/user/Projects/demo/art", HOME, &[], &kind).as_deref(),
            Some("/home/user/Projects/demo/art/anim")
        );
    }

    #[test]
    fn home_and_the_sidebar_roots_are_the_last_tries_d262() {
        let kind = kind_from(&["/home/user/Pictures", "/mnt/work/shared/art"], &[]);
        assert_eq!(
            resolve_typed_path("Pictures", "/tmp", HOME, &[], &kind).as_deref(),
            Some("/home/user/Pictures")
        );
        let roots = vec!["/mnt/work/shared".to_string()];
        assert_eq!(
            resolve_typed_path("art", "/tmp", HOME, &roots, &kind).as_deref(),
            Some("/mnt/work/shared/art")
        );
        assert_eq!(resolve_typed_path("", "/tmp", HOME, &roots, &kind), None);
        assert_eq!(resolve_typed_path("   ", "/tmp", HOME, &roots, &kind), None);
        assert_eq!(resolve_typed_path("no/such/thing", "/tmp", HOME, &roots, &kind), None);
    }
}

#[cfg(test)]
mod launch_arg_tests_d265 {
    use super::launch_path_in_args;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn a_bare_path_is_still_the_target_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["/home/user/notes.md"])),
            Some("/home/user/notes.md".to_string())
        );
    }

    #[test]
    fn select_names_the_file_outright_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["--select", "/home/user/notes.md"])),
            Some("/home/user/notes.md".to_string())
        );
        assert_eq!(
            launch_path_in_args(&args(&["--select=/home/user/notes.md"])),
            Some("/home/user/notes.md".to_string())
        );
    }

    #[test]
    fn select_wins_over_a_positional_path_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["/home/user", "--select", "/home/user/notes.md"])),
            Some("/home/user/notes.md".to_string())
        );
    }

    #[test]
    fn a_path_after_a_flag_is_found_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["--new-window", "/home/user/notes.md"])),
            Some("/home/user/notes.md".to_string())
        );
    }

    #[test]
    fn a_path_that_looks_like_a_flag_survives_a_double_dash_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["--", "--odd-name"])),
            Some("--odd-name".to_string())
        );
    }

    #[test]
    fn a_file_uri_is_decoded_either_way_d265() {
        assert_eq!(
            launch_path_in_args(&args(&["file:///home/user/my%20notes.md"])),
            Some("/home/user/my notes.md".to_string())
        );
        assert_eq!(
            launch_path_in_args(&args(&["--select", "file:///home/user/my%20notes.md"])),
            Some("/home/user/my notes.md".to_string())
        );
    }

    #[test]
    fn flags_alone_point_at_nothing_d265() {
        assert_eq!(launch_path_in_args(&args(&["--pick"])), None);
        assert_eq!(launch_path_in_args(&args(&["--select"])), None);
        assert_eq!(launch_path_in_args(&[]), None);
    }
}

#[cfg(test)]
mod tree_cache_tests {
    use super::*;

    fn kid(name: &str) -> Vec<TreeChild> {
        vec![(name.to_string(), format!("/x/{name}"), false)]
    }

    #[test]
    fn tree_cache_never_grows_past_its_cap_d295() {
        let mut cache = TreeCache::default();
        for n in 0..(TREE_CACHE_CAP + 250) {
            cache.insert_children(format!("/p/{n}"), kid("a"));
            cache.insert_openable(format!("/m/{n}"), true);
        }
        assert!(cache.children.len() <= TREE_CACHE_CAP);
        assert!(cache.openable.len() <= TREE_CACHE_CAP);
        // Oldest went first, newest stayed.
        assert!(cache.children("/p/0").is_none());
        assert!(cache.children(&format!("/p/{}", TREE_CACHE_CAP + 249)).is_some());
    }

    #[test]
    fn replacing_an_answer_does_not_age_it_twice_d295() {
        let mut cache = TreeCache::default();
        cache.insert_children("/a".into(), kid("1"));
        cache.insert_children("/a".into(), kid("2"));
        assert_eq!(cache.order.len(), 1, "one key, one slot in the order");
        assert_eq!(cache.children("/a").unwrap()[0].0, "2");
    }

    #[test]
    fn forget_under_drops_the_branch_and_nothing_beside_it_d295() {
        let mut cache = TreeCache::default();
        for path in ["/a", "/a/b", "/a/b/c", "/ab", "/z"] {
            cache.insert_children(path.into(), kid("k"));
        }
        cache.insert_openable("/a".into(), true);
        cache.forget_under("/a");
        assert!(cache.children("/a").is_none());
        assert!(cache.children("/a/b").is_none());
        assert!(cache.children("/a/b/c").is_none());
        assert!(cache.openable("/a").is_none());
        assert!(cache.children("/ab").is_some(), "a sibling that shares a prefix stays");
        assert!(cache.children("/z").is_some());
        assert_eq!(cache.order.len(), cache.children.len(), "the order never keeps a ghost");
    }

    #[test]
    fn a_changed_folder_forgets_itself_and_its_parent_only_d295() {
        let mut cache = TreeCache::default();
        for path in ["/", "/a", "/a/b", "/a/b/c"] {
            cache.insert_children(path.into(), kid("k"));
        }
        cache.forget_folder_and_parent("/a/b");
        assert!(cache.children("/a/b").is_none());
        assert!(cache.children("/a").is_none());
        assert!(cache.children("/a/b/c").is_some(), "grandchildren are untouched");
        assert!(cache.children("/").is_some());
        cache.forget_folder_and_parent("/a");
        assert!(cache.children("/").is_none(), "the parent of a top-level folder is the root");
    }

    #[test]
    fn clear_empties_everything_d295() {
        let mut cache = TreeCache::default();
        cache.insert_children("/a".into(), kid("k"));
        cache.insert_openable("/a".into(), false);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.order.is_empty());
    }

    #[test]
    fn sniff_names_the_real_type_d324() {
        assert_eq!(sniff_real_type(b"GIF89a....").unwrap().0, "a GIF image");
        assert_eq!(sniff_real_type(b"ID3\x03\0\0").unwrap().0, "MP3 audio");
        assert_eq!(sniff_real_type(&[0xFF, 0xFB, 0x90, 0x00]).unwrap().0, "MP3 audio");
        assert_eq!(sniff_real_type(b"RIFF\x00\x00\x00\x00WAVEfmt ").unwrap().0, "WAV audio");
        assert!(sniff_real_type(&[0, 1, 2, 3, 4, 5]).is_none());
        // A WAV whose payload is MPEG layer 3 is still a WAV on disk.
        assert!(sniff_real_type(b"RIFF\x00\x00\x00\x00WAVEfmt ").unwrap().1.contains(&"wav"));
    }

    #[test]
    fn damaged_reason_is_one_line_naming_the_real_type_d324() {
        let dir = scratch_root().join(format!("wf-d324c-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mp3_as_gif = dir.join("a.gif");
        fs::write(&mp3_as_gif, b"ID3\x03\0\0\0\0\0\0").unwrap();
        let reason = damaged_file_reason(mp3_as_gif.to_str().unwrap(), "picture");
        assert_eq!(reason, "This file is actually MP3 audio, not a .gif picture.");
        let gif_as_mp3 = dir.join("b.mp3");
        fs::write(&gif_as_mp3, b"GIF89a\0\0\0\0\0\0\0\0").unwrap();
        assert!(not_audio_reason(gif_as_mp3.to_str().unwrap()).unwrap().contains("actually a GIF image"));
        // Real audio, and a WAV named .mp3, are left for the player to try.
        let wav_as_mp3 = dir.join("c.mp3");
        fs::write(&wav_as_mp3, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
        assert!(not_audio_reason(wav_as_mp3.to_str().unwrap()).is_none());
        let real = dir.join("d.mp3");
        fs::write(&real, b"ID3\x03\0\0\0\0\0\0").unwrap();
        assert!(not_audio_reason(real.to_str().unwrap()).is_none());
        let empty = dir.join("e.mp3");
        fs::write(&empty, b"").unwrap();
        assert!(damaged_file_reason(empty.to_str().unwrap(), "audio file").starts_with("Empty file"));
        assert!(!reason.contains('\n'));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod midi_duration_tests {
    use super::*;

    fn vl(mut n: u32) -> Vec<u8> {
        let mut out = vec![(n & 0x7f) as u8];
        n >>= 7;
        while n > 0 {
            out.insert(0, ((n & 0x7f) as u8) | 0x80);
            n >>= 7;
        }
        out
    }
    fn track(events: &[u8]) -> Vec<u8> {
        let mut t = b"MTrk".to_vec();
        t.extend((events.len() as u32).to_be_bytes());
        t.extend(events);
        t
    }
    fn smf(format: u16, tpq: u16, tracks: &[Vec<u8>]) -> Vec<u8> {
        let mut f = b"MThd".to_vec();
        f.extend(6u32.to_be_bytes());
        f.extend(format.to_be_bytes());
        f.extend((tracks.len() as u16).to_be_bytes());
        f.extend(tpq.to_be_bytes());
        for t in tracks {
            f.extend(t);
        }
        f
    }
    const EOT: [u8; 3] = [0xFF, 0x2F, 0x00];

    /// 480 ticks per quarter, default tempo: 480 ticks = 0.5 s.
    fn one_second_track() -> Vec<u8> {
        let mut e = vec![0x00, 0x90, 60, 100];
        e.extend(vl(960));
        e.extend([0x80, 60, 0]);
        e.extend([0x00]);
        e.extend(EOT);
        track(&e)
    }

    #[test]
    fn plain_smf_length_uses_the_default_tempo() {
        let f = smf(0, 480, &[one_second_track()]);
        let d = midi_duration_from_bytes(&f).unwrap();
        assert!((d - 1.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn rmid_wrapped_file_reads_the_same_as_the_bare_one() {
        let inner = smf(0, 480, &[one_second_track()]);
        let mut riff = b"RIFF".to_vec();
        riff.extend((4 + 8 + inner.len() as u32).to_le_bytes());
        riff.extend(b"RMID");
        riff.extend(b"data");
        riff.extend((inner.len() as u32).to_le_bytes());
        riff.extend(&inner);
        let d = midi_duration_from_bytes(&riff).unwrap();
        assert!((d - 1.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn a_tempo_change_splits_the_arithmetic() {
        // Tempo track: 500000 until tick 480, then 1000000 (60 bpm).
        let mut t0 = vec![0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20];
        t0.extend(vl(480));
        t0.extend([0xFF, 0x51, 0x03, 0x0F, 0x42, 0x40]);
        t0.extend([0x00]);
        t0.extend(EOT);
        // Note track ends at tick 960: 0.5 s + 1.0 s.
        let mut t1 = vec![0x00, 0x90, 60, 100];
        t1.extend(vl(960));
        t1.extend([0x80, 60, 0, 0x00]);
        t1.extend(EOT);
        let f = smf(1, 480, &[track(&t0), track(&t1)]);
        let d = midi_duration_from_bytes(&f).unwrap();
        assert!((d - 1.5).abs() < 1e-9, "{d}");
    }

    #[test]
    fn running_status_and_sysex_are_walked_correctly() {
        let mut e = vec![0x00, 0x90, 60, 100];
        e.extend([0x00, 62, 100]); // running status note-on
        e.extend([0x00, 0xF0, 0x03, 1, 2, 0xF7]); // sysex
        e.extend(vl(480));
        e.extend([64, 100]); // running status after sysex is cancelled -> must be rejected
        let f = smf(0, 480, &[track(&e)]);
        assert_eq!(midi_duration_from_bytes(&f), None);

        let mut e = vec![0x00, 0x90, 60, 100];
        e.extend([0x00, 62, 100]);
        e.extend([0x00, 0xF0, 0x03, 1, 2, 0xF7]);
        e.extend(vl(480));
        e.extend([0x90, 64, 100]);
        e.extend(vl(480));
        e.extend([65, 100]); // running status, 960 ticks total = 1 s
        e.extend([0x00]);
        e.extend(EOT);
        let f = smf(0, 480, &[track(&e)]);
        let d = midi_duration_from_bytes(&f).unwrap();
        assert!((d - 1.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn truncated_and_garbage_input_is_none_and_never_panics() {
        assert_eq!(midi_duration_from_bytes(b""), None);
        assert_eq!(midi_duration_from_bytes(b"MThd"), None);
        assert_eq!(midi_duration_from_bytes(b"not a midi file at all, really"), None);
        let good = smf(0, 480, &[one_second_track()]);
        for cut in 0..good.len() {
            let _ = midi_duration_from_bytes(&good[..cut]);
        }
        // Every single-byte corruption must also be survivable.
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] = 0xFF;
            let _ = midi_duration_from_bytes(&bad);
        }
        let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let _ = midi_duration_from_bytes(&junk);
        assert_eq!(midi_duration_secs("/nonexistent/none.mid"), None);
    }

    #[test]
    fn midi_extension_check_ignores_case() {
        for name in ["a.mid", "a.MID", "a.Midi", "a.kar", "a.RMI"] {
            assert!(is_midi_path(name), "{name}");
        }
        assert!(!is_midi_path("a.mp3"));
    }

    /// Seconds of audio in a canonical PCM WAV, from its fmt and data chunks.
    fn wav_secs(path: &str) -> Option<f64> {
        let b = fs::read(path).ok()?;
        let (mut rate, mut pos) = (0u32, 12usize);
        while pos + 8 <= b.len() {
            let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
            match &b[pos..pos + 4] {
                b"fmt " => rate = u32::from_le_bytes(b[pos + 16..pos + 20].try_into().ok()?),
                b"data" if rate > 0 => return Some(len.min(b.len() - pos - 8) as f64 / rate as f64),
                _ => {}
            }
            pos += 8 + len + (len & 1);
        }
        None
    }

    #[test]
    fn real_songs_match_their_wav_renders_when_present() {
        // Read the folder from $HOME so no username sits in a tracked file; skip if HOME is unset.
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let dir = format!("{home}/Music/For Suno");
        for song in ["song1", "Song2", "song3", "song4", "song7", "song8", "song9", "song10", "song11", "lara2"] {
            let mid = format!("{dir}/{song}.mid");
            let wav = format!("{dir}/{song}.wav");
            let (Some(w), true) = (wav_secs(&wav), std::path::Path::new(&mid).exists()) else {
                continue;
            };
            let m = midi_duration_secs(&mid);
            eprintln!("MIDI-VS-WAV {song}: midi {m:?}s, wav {w:.2}s");
            let m = m.expect("a real song must parse");
            // The render adds a release tail of a few seconds, never shortens.
            assert!(w - m > -1.0 && w - m < 6.0, "{song}: midi {m} wav {w}");
        }
    }
}

#[cfg(test)]
mod select_launch_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = scratch_root().join(format!("wf-d328-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Images/Animals")).unwrap();
        fs::write(dir.join("Images/a.txt"), "x").unwrap();
        dir
    }

    #[test]
    fn a_folder_opens_its_parent_with_the_folder_highlighted_d328() {
        let dir = scratch("folder");
        let target = dir.join("Images");
        let got = select_launch_target(target.to_str().unwrap());
        assert_eq!(got.folder, dir.to_string_lossy());
        assert_eq!(got.select.as_deref(), target.to_str());
        // A trailing slash is the same folder.
        let slash = select_launch_target(&format!("{}/", target.display()));
        assert_eq!(slash.folder, dir.to_string_lossy());
        assert!(slash.select.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_opens_its_folder_with_the_file_highlighted_d328() {
        let dir = scratch("file");
        let target = dir.join("Images/a.txt");
        let got = select_launch_target(target.to_str().unwrap());
        assert_eq!(got.folder, dir.join("Images").to_string_lossy());
        assert_eq!(got.select.as_deref(), target.to_str());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_path_opens_the_nearest_folder_that_exists_d328() {
        let dir = scratch("missing");
        let target = dir.join("Images/Gone/deeper/file.png");
        let got = select_launch_target(target.to_str().unwrap());
        assert_eq!(got.folder, dir.join("Images").to_string_lossy());
        assert_eq!(got.select, None);
        // Nothing of it exists at all: the root, still a window.
        let got = select_launch_target("/definitely-not-here-d328/x/y");
        assert_eq!((got.folder.as_str(), got.select), ("/", None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_root_has_no_parent_and_opens_itself_d328() {
        let got = select_launch_target("/");
        assert_eq!((got.folder.as_str(), got.select), ("/", None));
    }
}
