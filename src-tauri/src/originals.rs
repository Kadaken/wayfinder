//! "Show original": where Wayfinder learns that a file is a copy of another.
//!
//! Two sources. Hand lists (TSV: `duplicate<TAB>original<TAB>how we know`) and
//! a background index of byte-identical files (same size, same blake2b hash).
//!
//! Privacy and safety rules, all enforced here:
//! - the index is off unless the config names roots to index, and it only
//!   walks those roots;
//! - a skipped folder is checked BEFORE it is opened: it is never listed,
//!   read or stat-ed inside;
//! - trash folders are always skipped and symlinks are never followed;
//! - the only thing ever written is the index's own cache file.

use blake2::{Blake2b512, Digest};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// One row of a hand list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRow {
    pub duplicate: String,
    pub original: String,
    pub how: String,
}

/// Parse a hand list. The header line, blank lines, rows with fewer than two
/// columns and rows whose paths are not absolute are ignored.
pub fn parse_originals_tsv(text: &str) -> Vec<ListedRow> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let duplicate = cols.next().unwrap_or("").trim();
        let original = cols.next().unwrap_or("").trim();
        let how = cols.next().unwrap_or("").trim();
        if i == 0 && duplicate == "duplicate" && original == "original" {
            continue;
        }
        if !duplicate.starts_with('/') || !original.starts_with('/') {
            continue;
        }
        rows.push(ListedRow {
            duplicate: duplicate.to_string(),
            original: original.to_string(),
            how: how.to_string(),
        });
    }
    rows
}

type ListCache = HashMap<PathBuf, (SystemTime, Arc<HashMap<String, ListedRow>>)>;

fn list_cache() -> &'static Mutex<ListCache> {
    static CACHE: OnceLock<Mutex<ListCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A hand list as a map duplicate -> row, re-read only when its modification
/// time changed.
fn load_list(path: &Path) -> Option<Arc<HashMap<String, ListedRow>>> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    if let Ok(cache) = list_cache().lock() {
        if let Some((seen, map)) = cache.get(path) {
            if *seen == mtime {
                return Some(map.clone());
            }
        }
    }
    let text = std::fs::read_to_string(path).ok()?;
    let map: HashMap<String, ListedRow> = parse_originals_tsv(&text)
        .into_iter()
        .map(|row| (row.duplicate.clone(), row))
        .collect();
    let map = Arc::new(map);
    if let Ok(mut cache) = list_cache().lock() {
        cache.insert(path.to_path_buf(), (mtime, map.clone()));
    }
    Some(map)
}

/// The original of `file` from the hand lists, if one is listed and still
/// exists.
pub fn listed_original(file: &str, lists: &[String]) -> Option<ListedRow> {
    for list in lists {
        if let Some(map) = load_list(Path::new(list)) {
            if let Some(row) = map.get(file) {
                if Path::new(&row.original).exists() {
                    return Some(row.clone());
                }
            }
        }
    }
    None
}

// ---- the walk ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRec {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
}

/// Built-in trash rule: `.Trash`, `.Trash-<uid>` and anything named `Trash`
/// under `.local/share`, plus any extra trash path given.
pub fn is_trash_dir(path: &Path, extra: &[PathBuf]) -> bool {
    if extra.iter().any(|t| path == t.as_path()) {
        return true;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name == ".Trash" || name.starts_with(".Trash-") {
        return true;
    }
    if name == "Trash" {
        let parent = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
        let grand = path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
        return parent.as_deref() == Some("share") && grand.as_deref() == Some(".local");
    }
    false
}

pub fn is_skipped(path: &Path, skip: &[PathBuf], trash: &[PathBuf]) -> bool {
    skip.iter().any(|s| path.starts_with(s)) || is_trash_dir(path, trash)
}

/// Walk `roots`, returning every regular file. `on_open` is called with each
/// directory immediately before it is opened; a skipped directory never gets
/// that call. Symlinks are neither followed nor listed.
pub fn walk_roots(
    roots: &[PathBuf],
    skip: &[PathBuf],
    trash: &[PathBuf],
    on_open: &mut dyn FnMut(&Path),
    should_stop: &dyn Fn() -> bool,
) -> Vec<FileRec> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.iter().filter(|r| !is_skipped(r, skip, trash)).cloned().collect();
    while let Some(dir) = stack.pop() {
        if should_stop() {
            break;
        }
        on_open(&dir);
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !is_skipped(&path, skip, trash) {
                    stack.push(path);
                }
            } else if kind.is_file() {
                let Ok(meta) = entry.metadata() else { continue };
                let Some(text) = path.to_str() else { continue };
                if text.contains('\t') || text.contains('\n') {
                    continue;
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.push(FileRec { path: text.to_string(), size: meta.len(), mtime });
            }
        }
    }
    out
}

// ---- grouping and hashing --------------------------------------------------

/// Only files whose size matches another file's can be identical. Empty files
/// are left out: they are all "identical" and prove nothing.
pub fn candidates_by_size(files: &[FileRec]) -> Vec<&FileRec> {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for f in files {
        *counts.entry(f.size).or_insert(0) += 1;
    }
    files.iter().filter(|f| f.size > 0 && counts[&f.size] > 1).collect()
}

/// Of a set of identical files, the original is the one with the oldest
/// modification time; a tie goes to the smaller path, so the answer is stable.
pub fn pick_original(group: &[FileRec]) -> Option<&FileRec> {
    group.iter().min_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.path.cmp(&b.path)))
}

/// Size groups first, then hash only the files that share a size. `hash_of`
/// is given each candidate and returns its hash (from the cache or by reading
/// the file). Returns map duplicate path -> original path.
pub fn find_duplicates(
    files: &[FileRec],
    hash_of: &mut dyn FnMut(&FileRec) -> Option<String>,
    should_stop: &dyn Fn() -> bool,
) -> HashMap<String, String> {
    let mut by_hash: HashMap<(u64, String), Vec<FileRec>> = HashMap::new();
    for f in candidates_by_size(files) {
        if should_stop() {
            return HashMap::new();
        }
        if let Some(h) = hash_of(f) {
            by_hash.entry((f.size, h)).or_default().push(f.clone());
        }
    }
    let mut out = HashMap::new();
    for group in by_hash.values().filter(|g| g.len() > 1) {
        if let Some(orig) = pick_original(group) {
            for f in group {
                if f.path != orig.path {
                    out.insert(f.path.clone(), orig.path.clone());
                }
            }
        }
    }
    out
}

pub fn hash_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Blake2b512::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ---- cache file ------------------------------------------------------------

/// path -> (size, mtime, hash)
pub type HashCache = HashMap<String, (u64, i64, String)>;

pub fn parse_cache(text: &str) -> HashCache {
    let mut out = HashCache::new();
    for line in text.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() == 4 {
            if let (Ok(size), Ok(mtime)) = (c[1].parse(), c[2].parse()) {
                out.insert(c[0].to_string(), (size, mtime, c[3].to_string()));
            }
        }
    }
    out
}

pub fn render_cache(cache: &HashCache) -> String {
    let mut lines: Vec<String> = cache
        .iter()
        .map(|(p, (s, m, h))| format!("{p}\t{s}\t{m}\t{h}"))
        .collect();
    lines.sort();
    lines.join("\n") + "\n"
}

/// `$XDG_CACHE_HOME/wayfinder/duplicate-index.tsv`, else `$HOME/.cache/...`.
pub fn cache_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(p) => p,
        None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("wayfinder").join("duplicate-index.tsv"))
}

/// Run the whole index once: walk, hash what needs hashing (reusing the cache
/// where size and mtime still match), save the cache, return duplicate ->
/// original.
pub fn build_index(
    roots: &[PathBuf],
    skip: &[PathBuf],
    trash: &[PathBuf],
    cache_file: Option<&Path>,
    should_stop: &dyn Fn() -> bool,
) -> HashMap<String, String> {
    let files = walk_roots(roots, skip, trash, &mut |_| {}, should_stop);
    let old = cache_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_cache(&t))
        .unwrap_or_default();
    let mut fresh = HashCache::new();
    let result = find_duplicates(
        &files,
        &mut |f| {
            if let Some((size, mtime, hash)) = old.get(&f.path) {
                if *size == f.size && *mtime == f.mtime {
                    fresh.insert(f.path.clone(), (*size, *mtime, hash.clone()));
                    return Some(hash.clone());
                }
            }
            let hash = hash_file(Path::new(&f.path))?;
            fresh.insert(f.path.clone(), (f.size, f.mtime, hash.clone()));
            // Be gentle with the drive: a short pause after each real read.
            std::thread::sleep(std::time::Duration::from_millis(2));
            Some(hash)
        },
        should_stop,
    );
    if !should_stop() {
        if let Some(path) = cache_file {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
                let tmp = path.with_extension("tsv.tmp");
                if std::fs::write(&tmp, render_cache(&fresh)).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
    }
    result
}

// ---- process-wide state ------------------------------------------------------

fn index_slot() -> &'static Mutex<Option<Arc<HashMap<String, String>>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<HashMap<String, String>>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

static STARTED: OnceLock<()> = OnceLock::new();

/// Start the background index once per process. Does nothing when no roots
/// are configured. Runs on its own thread at the lowest CPU priority.
pub fn start_index_once(roots: Vec<String>, skip: Vec<String>) {
    if roots.is_empty() || STARTED.set(()).is_err() {
        return;
    }
    let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).filter(|p| p.is_absolute()).collect();
    let skip: Vec<PathBuf> = skip.into_iter().map(PathBuf::from).collect();
    let trash = system_trash_dirs();
    let _ = std::thread::Builder::new().name("wf-dup-index".into()).spawn(move || {
        // On Linux this lowers only this thread's niceness.
        let _ = rustix::process::setpriority_process(None, 19);
        let map = build_index(&roots, &skip, &trash, cache_path().as_deref(), &|| false);
        if let Ok(mut slot) = index_slot().lock() {
            *slot = Some(Arc::new(map));
        }
    });
}

pub fn system_trash_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        out.push(data.join("Trash"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(home).join(".local/share/Trash"));
    }
    out
}

/// The byte-identical original of `file`, if the index knows one that still exists.
pub fn indexed_original(file: &str) -> Option<String> {
    let map = index_slot().lock().ok()?.clone()?;
    let orig = map.get(file)?;
    Path::new(orig).exists().then(|| orig.clone())
}

#[cfg(test)]
thread_local! {
    static TEST_LISTS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Test hook: make `original_for` use these hand lists instead of the config.
#[cfg(test)]
pub fn set_test_lists(lists: Option<Vec<String>>) {
    TEST_LISTS.with(|t| *t.borrow_mut() = lists);
}

#[cfg(test)]
fn lists_override() -> Option<Vec<String>> {
    TEST_LISTS.with(|t| t.borrow().clone())
}

#[cfg(not(test))]
#[inline(always)]
fn lists_override() -> Option<Vec<String>> {
    None
}

/// What the menu shows for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub path: String,
    pub tooltip: String,
}

/// The original of `file`: hand lists first, then the hash index.
pub fn original_for(file: &str, config_lists: &[String]) -> Option<Original> {
    let lists = lists_override().unwrap_or_else(|| config_lists.to_vec());
    if let Some(row) = listed_original(file, &lists) {
        let how = if row.how.is_empty() { String::new() } else { format!(" ({})", row.how) };
        return Some(Original {
            tooltip: format!("Open the original in a new window: {}{how}", row.original),
            path: row.original,
        });
    }
    indexed_original(file).map(|path| Original {
        tooltip: format!("Byte-identical copy. The older file is treated as the original: {path}"),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wf-orig-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn originals_tsv_parser_skips_header_blank_and_bad_rows() {
        let text = "duplicate\toriginal\thow we know\n\
                    /a/dup1.mp4\t/b/full.mp4\tcut of full video\n\
                    \n\
                    relative.mp4\t/b/full.mp4\tnot absolute\n\
                    /a/dup2.mp4\t/b/full2.mp4\n\
                    /only/one/column\n\
                    /a/dup3.mp4\t/b/full3.mp4\tgot\ttabs in note\r\n";
        let rows = parse_originals_tsv(text);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ListedRow { duplicate: "/a/dup1.mp4".into(), original: "/b/full.mp4".into(), how: "cut of full video".into() });
        assert_eq!(rows[1].how, "");
        assert_eq!(rows[2].how, "got\ttabs in note");
    }

    #[test]
    fn listed_original_ignores_missing_originals_and_follows_edits() {
        let dir = fixture("list");
        let orig = dir.join("full.bin");
        let dup = dir.join("cut.bin");
        std::fs::write(&orig, "x").unwrap();
        std::fs::write(&dup, "y").unwrap();
        let list = dir.join("o.tsv");
        std::fs::write(&list, format!("duplicate\toriginal\thow we know\n{}\t{}\tby hand\n", dup.display(), orig.display())).unwrap();
        let lists = vec![list.to_string_lossy().to_string()];
        let found = listed_original(dup.to_str().unwrap(), &lists).expect("listed");
        assert_eq!(found.original, orig.to_str().unwrap());
        assert!(listed_original(orig.to_str().unwrap(), &lists).is_none(), "the original has no original");
        std::fs::remove_file(&orig).unwrap();
        assert!(listed_original(dup.to_str().unwrap(), &lists).is_none(), "a vanished original is ignored");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_files_with_a_matching_size_are_hashed() {
        let rec = |p: &str, size: u64, mtime: i64| FileRec { path: p.into(), size, mtime };
        let files = vec![rec("/a", 10, 1), rec("/b", 10, 2), rec("/c", 11, 1), rec("/d", 0, 1), rec("/e", 0, 2)];
        let hashed = RefCell::new(Vec::new());
        let result = find_duplicates(
            &files,
            &mut |f| {
                hashed.borrow_mut().push(f.path.clone());
                Some("same".to_string())
            },
            &|| false,
        );
        let mut hashed = hashed.into_inner();
        hashed.sort();
        assert_eq!(hashed, vec!["/a", "/b"], "unique sizes and empty files are never hashed");
        assert_eq!(result.get("/b").map(String::as_str), Some("/a"));
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let dir = fixture("hash");
        std::fs::write(dir.join("a"), "aaaa").unwrap();
        std::fs::write(dir.join("b"), "bbbb").unwrap();
        std::fs::write(dir.join("c"), "aaaa").unwrap();
        let map = build_index(&[dir.clone()], &[], &[], None, &|| false);
        let a = dir.join("a").to_string_lossy().to_string();
        let c = dir.join("c").to_string_lossy().to_string();
        let b = dir.join("b").to_string_lossy().to_string();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&a) || map.contains_key(&c));
        assert!(!map.contains_key(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_older_file_is_the_original() {
        let rec = |p: &str, mtime: i64| FileRec { path: p.into(), size: 5, mtime };
        let group = vec![rec("/new", 200), rec("/old", 100), rec("/mid", 150)];
        assert_eq!(pick_original(&group).unwrap().path, "/old");
        let tie = vec![rec("/b", 100), rec("/a", 100)];
        assert_eq!(pick_original(&tie).unwrap().path, "/a", "a tie goes to the smaller path");
    }

    #[test]
    fn a_skipped_folder_is_never_opened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture("skip");
        let keep = dir.join("keep");
        let skipped = dir.join("private");
        let inside = skipped.join("deep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(keep.join("f"), "data").unwrap();
        std::fs::write(inside.join("g"), "data").unwrap();
        std::fs::set_permissions(&skipped, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut opened: Vec<PathBuf> = Vec::new();
        let files = walk_roots(&[dir.clone()], &[skipped.clone()], &[], &mut |d| opened.push(d.to_path_buf()), &|| false);
        std::fs::set_permissions(&skipped, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(opened.contains(&keep), "the normal folder is opened");
        assert!(opened.iter().all(|d| !d.starts_with(&skipped)), "nothing under the skipped folder was opened: {opened:?}");
        assert_eq!(files.len(), 1);
        assert!(files[0].path.ends_with("keep/f"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_folders_and_symlinks_are_skipped() {
        let dir = fixture("trash");
        for d in [".Trash-1000", ".Trash", ".local/share/Trash", "ok"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            std::fs::write(dir.join(d).join("f"), "data").unwrap();
        }
        let outside = fixture("outside");
        std::fs::write(outside.join("secret"), "data").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), dir.join("filelink")).unwrap();
        let files = walk_roots(&[dir.clone()], &[], &[], &mut |_| {}, &|| false);
        assert_eq!(files.len(), 1, "only the plain folder's file: {files:?}");
        assert!(files[0].path.ends_with("ok/f"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn index_reuses_its_cache_and_writes_only_that_file() {
        let dir = fixture("cache");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("a"), "same").unwrap();
        std::fs::write(data.join("b"), "same").unwrap();
        let cache = dir.join("cache").join("idx.tsv");
        let first = build_index(&[data.clone()], &[], &[], Some(&cache), &|| false);
        assert_eq!(first.len(), 1);
        let cached = parse_cache(&std::fs::read_to_string(&cache).unwrap());
        assert_eq!(cached.len(), 2);
        let second = build_index(&[data.clone()], &[], &[], Some(&cache), &|| false);
        assert_eq!(first, second);
        let names: Vec<_> = std::fs::read_dir(&data).unwrap().flatten().collect();
        assert_eq!(names.len(), 2, "nothing was written into the indexed root");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
