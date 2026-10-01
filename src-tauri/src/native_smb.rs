//! D-156 (connect): SMB address parsing, remembered locations and the async
//! connect flow. Everything here is GTK-free: the network is reached only
//! through the `SmbBackend` trait, so tests drive the whole flow with a fake.

use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

/// Fixed status texts. Backend error text is never shown: it can carry a
/// user name, a password prompt answer or a raw URI.
pub const INVALID_ADDRESS: &str = "Enter an address like smb://server/share or \\\\server\\share.";
const MAX_REMEMBERED: usize = 20;
/// The chooser shows at most this many shares, cut after dedup and sorting.
const MAX_SHARES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmbLocation {
    pub host: String,
    pub share: Option<String>,
    pub subpath: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmbInputError {
    Empty,
    Scheme,
    Host,
    Credentials,
    Control,
    Escape,
    Dots,
    Malformed,
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Strict percent-decoding: every `%` needs two hex digits and the result
/// must be UTF-8.
fn percent_decode(part: &str) -> Result<String, SmbInputError> {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).copied().and_then(hex).ok_or(SmbInputError::Escape)?;
            let lo = bytes.get(i + 2).copied().and_then(hex).ok_or(SmbInputError::Escape)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| SmbInputError::Escape)
}

/// Component-safe encoding: everything except RFC 3986 unreserved bytes.
fn percent_encode(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for byte in part.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('.')
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn check_component(part: String) -> Result<String, SmbInputError> {
    if part.chars().any(|c| c.is_control()) {
        return Err(SmbInputError::Control);
    }
    if part == "." || part == ".." {
        return Err(SmbInputError::Dots);
    }
    if part.is_empty() || part.contains('/') || part.contains('\\') {
        return Err(SmbInputError::Malformed);
    }
    Ok(part)
}

/// True for text that is meant as a network address, so the location field
/// routes it to the connect flow instead of the local filesystem.
pub fn looks_like_smb(text: &str) -> bool {
    let text = text.trim();
    strip_prefix_ci(text, "smb://").is_some() || text.starts_with("\\\\")
}

/// `smb://server[/share[/path]]` and `\\server[\share[\path]]` in, one
/// credential-free location out. Nothing here touches the network.
pub fn parse_smb_input(raw: &str) -> Result<SmbLocation, SmbInputError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(SmbInputError::Empty);
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(SmbInputError::Control);
    }
    let (rest, sep, is_uri) = if let Some(rest) = strip_prefix_ci(text, "smb://") {
        (rest, '/', true)
    } else if let Some(rest) = text.strip_prefix("\\\\") {
        (rest, '\\', false)
    } else {
        return Err(SmbInputError::Scheme);
    };
    if is_uri && (rest.contains('?') || rest.contains('#')) {
        return Err(SmbInputError::Malformed);
    }
    if !is_uri && rest.contains('/') {
        return Err(SmbInputError::Malformed);
    }
    let mut parts: Vec<&str> = rest.split(sep).collect();
    while parts.len() > 1 && parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    let host_raw = parts[0];
    if host_raw.contains('@') {
        return Err(SmbInputError::Credentials);
    }
    if !valid_host(host_raw) {
        return Err(SmbInputError::Host);
    }
    let mut names = Vec::new();
    for part in &parts[1..] {
        let decoded = if is_uri { percent_decode(part)? } else { (*part).to_string() };
        names.push(check_component(decoded)?);
    }
    let mut names = names.into_iter();
    Ok(SmbLocation {
        host: host_raw.to_ascii_lowercase(),
        share: names.next(),
        subpath: names.collect(),
    })
}

impl SmbLocation {
    /// The server root, used to list shares.
    pub fn server_uri(&self) -> String {
        format!("smb://{}/", self.host)
    }

    /// Server and share only: the one thing that is mounted and remembered.
    pub fn share_uri(&self) -> Option<String> {
        self.share.as_ref().map(|share| format!("smb://{}/{}", self.host, percent_encode(share)))
    }

    /// A plain name that never carries anything but the host and share.
    pub fn label(&self) -> String {
        match &self.share {
            Some(share) => format!("{share} on {}", self.host),
            None => self.host.clone(),
        }
    }
}

/// One immediate child of a server root: `name` is the protocol identity the
/// URI is built from; `display` is presentation text only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareEntry {
    pub name: String,
    pub display: String,
}

impl ShareEntry {
    pub fn plain(name: &str) -> Self {
        Self { name: name.to_string(), display: name.to_string() }
    }
}

/// The exact share name, or `None` when it could not be a safe URI component.
/// Administrative shares (`C$`, `IPC$`) are kept: the server exposes them.
pub fn share_identity(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && name.chars().count() <= 255
        && name != "."
        && name != ".."
        && !name.trim().is_empty()
        && !name.chars().any(|c| c.is_control() || c == '/' || c == '\\');
    ok.then(|| name.to_string())
}

/// Presentation text: no control characters, trimmed, at most 80 characters.
pub fn display_label(text: &str) -> Option<String> {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
    let cleaned: String = cleaned.trim().chars().take(80).collect();
    let lower = cleaned.to_lowercase();
    // URI, userinfo or password-shaped display text is never shown
    let shaped = lower.contains("://") || lower.contains('@') || lower.contains("password");
    (!cleaned.is_empty() && !shaped).then_some(cleaned)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareChoice {
    /// Sanitized display text only.
    pub name: String,
    pub uri: String,
}

/// Immediate shares of `host`. URIs come from the actual child names.
/// Deduplicated by canonical URI and sorted by a total order, so the result
/// (and the cut at `MAX_SHARES`) does not depend on server enumeration order.
pub fn share_choices(host: &str, entries: &[ShareEntry]) -> Vec<ShareChoice> {
    let mut all: Vec<ShareChoice> = Vec::new();
    for entry in entries {
        let Some(name) = share_identity(&entry.name) else { continue };
        let uri = format!("smb://{host}/{}", percent_encode(&name));
        let label = display_label(&entry.display).or_else(|| display_label(&name)).unwrap_or_else(|| "Share".into());
        all.push(ShareChoice { name: label, uri });
    }
    all.sort_by(|a, b| a.uri.to_lowercase().cmp(&b.uri.to_lowercase()).then(a.uri.cmp(&b.uri)).then(a.name.cmp(&b.name)));
    all.dedup_by(|b, a| a.uri.eq_ignore_ascii_case(&b.uri));
    all.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.name.cmp(&b.name)).then(a.uri.cmp(&b.uri))
    });
    all.truncate(MAX_SHARES);
    all
}

/// One GVfs aggregate child that is an SMB share: host and share are decoded
/// backend identity, never a friendly label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedShare {
    pub host: String,
    pub share: String,
    pub path: PathBuf,
}

/// What one local-only lookup found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(PathBuf),
    /// Nothing visible yet: gvfsd-fuse may not have published the child.
    NotYet,
    /// Ambiguous, or the subpath is missing or escapes: final.
    Fail,
}

/// The mounted root for `loc`, by name only, with no filesystem metadata call:
/// a native absolute path first, else exactly one GVfs child with the same
/// host and share (case-insensitive). Several distinct matches are `Fail`.
pub fn lookup_root(native: Option<&Path>, gvfs: &dyn Fn() -> Vec<MountedShare>, loc: &SmbLocation) -> Lookup {
    if let Some(path) = native.filter(|p| p.is_absolute()) {
        return Lookup::Found(path.to_path_buf());
    }
    let Some(share) = loc.share.as_ref() else { return Lookup::Fail };
    let mut hits: Vec<PathBuf> = gvfs()
        .into_iter()
        .filter(|m| m.host.eq_ignore_ascii_case(&loc.host) && m.share.to_lowercase() == share.to_lowercase())
        .filter(|m| m.path.is_absolute())
        .map(|m| m.path)
        .collect();
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => Lookup::NotYet,
        1 => Lookup::Found(hits.remove(0)),
        _ => Lookup::Fail,
    }
}

/// `lookup_root`, then the subpath. The subpath touches the filesystem, so
/// this whole function runs on a worker, never on the GTK thread. A share-only
/// request does no filesystem work at all.
pub fn lookup_full(native: Option<&Path>, gvfs: &dyn Fn() -> Vec<MountedShare>, loc: &SmbLocation) -> Lookup {
    match lookup_root(native, gvfs, loc) {
        Lookup::Found(root) if loc.subpath.is_empty() => Lookup::Found(root),
        Lookup::Found(root) => descend(&root, &loc.subpath).map_or(Lookup::Fail, Lookup::Found),
        other => other,
    }
}

/// Where lookups run and where retries wait. Production: a GIO worker thread
/// and a GLib timer. Tests: a manual queue.
pub trait Sched {
    /// Runs `job` off the GTK thread, then `then` on it.
    fn work(&self, job: Box<dyn FnOnce() -> Lookup + Send>, then: Box<dyn FnOnce(Lookup)>);
    fn after(&self, millis: u32, then: Box<dyn FnOnce()>);
}

/// The explicit retry bound: 8 lookups, 400 ms apart (about 2.8 s of waiting).
pub const RESOLVE_ATTEMPTS: u32 = 8;
pub const RESOLVE_RETRY_MS: u32 = 400;

/// Runs `lookup` on a worker; while nothing is visible yet it retries on a
/// timer, up to `RESOLVE_ATTEMPTS`. It never remounts. When `alive` turns false
/// the chain stops and `done` is never called.
pub fn resolve_async(
    sched: Rc<dyn Sched>,
    alive: Rc<dyn Fn() -> bool>,
    lookup: Arc<dyn Fn() -> Lookup + Send + Sync>,
    done: Box<dyn FnOnce(Option<PathBuf>)>,
) {
    attempt(sched, alive, lookup, done, 1);
}

fn attempt(
    sched: Rc<dyn Sched>,
    alive: Rc<dyn Fn() -> bool>,
    lookup: Arc<dyn Fn() -> Lookup + Send + Sync>,
    done: Box<dyn FnOnce(Option<PathBuf>)>,
    n: u32,
) {
    if !alive() {
        return;
    }
    let job = {
        let lookup = lookup.clone();
        Box::new(move || lookup()) as Box<dyn FnOnce() -> Lookup + Send>
    };
    let next = sched.clone();
    sched.work(
        job,
        Box::new(move |found| {
            if !alive() {
                return;
            }
            match found {
                Lookup::Found(path) => done(Some(path)),
                Lookup::Fail => done(None),
                Lookup::NotYet if n >= RESOLVE_ATTEMPTS => done(None),
                Lookup::NotYet => {
                    let again = next.clone();
                    next.after(
                        RESOLVE_RETRY_MS,
                        Box::new(move || attempt(again, alive, lookup, done, n + 1)),
                    );
                }
            }
        }),
    );
}


/// One remembered share. Only the canonical share URI and a plain label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedNetwork {
    pub uri: String,
    pub label: String,
}

/// An entry read from a hand-edited config is used only if it is exactly a
/// canonical credential-free share URI.
pub fn remembered_location(entry: &RememberedNetwork) -> Option<SmbLocation> {
    let loc = parse_smb_input(&entry.uri).ok()?;
    (loc.share.is_some() && loc.subpath.is_empty() && loc.share_uri().as_deref() == Some(entry.uri.as_str()))
        .then_some(loc)
}

/// The label shown for a saved entry: always derived from its validated URI.
/// A stored label is untrusted and never displayed.
pub fn remembered_label(entry: &RememberedNetwork) -> Option<String> {
    remembered_location(entry).map(|loc| loc.label())
}

/// Rewrites the stored label of every valid entry from its URI; entries that
/// are not valid are left exactly as they are.
fn rederive_labels(list: &mut [RememberedNetwork]) {
    for entry in list.iter_mut() {
        if let Some(label) = remembered_label(entry) {
            entry.label = label;
        }
    }
}

/// Adds (or moves to the front) one share. Returns false when `loc` is not a share.
pub fn remember(list: &mut Vec<RememberedNetwork>, loc: &SmbLocation) -> bool {
    let Some(uri) = loc.share_uri() else { return false };
    list.retain(|e| !e.uri.eq_ignore_ascii_case(&uri));
    let label = SmbLocation { host: loc.host.clone(), share: loc.share.clone(), subpath: Vec::new() }.label();
    list.insert(0, RememberedNetwork { uri, label });
    list.truncate(MAX_REMEMBERED);
    rederive_labels(list);
    true
}

/// Removes exactly the entry whose URI matches. Returns whether one was removed.
pub fn forget(list: &mut Vec<RememberedNetwork>, uri: &str) -> bool {
    let before = list.len();
    list.retain(|e| !e.uri.eq_ignore_ascii_case(uri));
    rederive_labels(list);
    list.len() != before
}

/// The mounted root plus `subpath`, only if it exists, is a directory and
/// stays under the mounted root after symlinks resolve.
pub fn descend(root: &Path, subpath: &[String]) -> Option<PathBuf> {
    if !root.is_absolute() || !root.is_dir() {
        return None;
    }
    if subpath.is_empty() {
        return Some(root.to_path_buf());
    }
    let mut target = root.to_path_buf();
    for part in subpath {
        target.push(part);
    }
    let real_root = std::fs::canonicalize(root).ok()?;
    let real_target = std::fs::canonicalize(&target).ok()?;
    (real_target.starts_with(&real_root) && real_target.is_dir()).then_some(target)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmbFail {
    Cancelled,
    Denied,
    Unreachable,
    NotSupported,
    Other,
}

impl SmbFail {
    pub fn text(self, label: &str) -> String {
        match self {
            SmbFail::Cancelled => "Connection cancelled.".to_string(),
            SmbFail::Denied => format!("Access to {label} was denied."),
            SmbFail::Unreachable => format!("Could not reach {label}."),
            SmbFail::NotSupported => "Network-share support (gvfs-smb) is not installed.".to_string(),
            SmbFail::Other => format!("Could not connect to {label}."),
        }
    }
}

/// The network. `done` may run at any later time, or never.
pub trait SmbBackend {
    /// Immediate share names of a server root. An error after some names
    /// arrived is reported as `Ok` with the partial list.
    fn list_shares(&self, server_uri: &str, done: Box<dyn FnOnce(Result<Vec<ShareEntry>, SmbFail>)>);
    /// `Ok` also for "already mounted".
    fn mount_share(&self, share_uri: &str, done: Box<dyn FnOnce(Result<(), SmbFail>)>);
    /// After a successful mount: the one local folder for `share_uri` plus
    /// `subpath`, found without blocking the GTK thread and cancellable by
    /// `cancel`. `done(None)` means it could not be resolved. Never remounts.
    fn resolve(&self, share_uri: &str, subpath: &[String], done: Box<dyn FnOnce(Option<PathBuf>)>);
    /// Cancels whatever is in flight.
    fn cancel(&self);
}

/// What the flow reports back to the window.
pub trait SmbSink {
    fn busy(&self, on: bool);
    fn status(&self, text: &str);
    fn shares(&self, server: &str, choices: Vec<ShareChoice>);
    /// A verified local directory: remember, refresh drives and navigate.
    fn opened(&self, local: &Path, share: &SmbLocation);
}

pub struct SmbFlow {
    backend: Rc<dyn SmbBackend>,
    sink: Rc<dyn SmbSink>,
    generation: Cell<u64>,
    busy: Cell<bool>,
    this: RefCell<Weak<SmbFlow>>,
}

impl SmbFlow {
    pub fn new(backend: Rc<dyn SmbBackend>, sink: Rc<dyn SmbSink>) -> Rc<Self> {
        let flow = Rc::new(Self {
            backend,
            sink,
            generation: Cell::new(0),
            busy: Cell::new(false),
            this: RefCell::new(Weak::new()),
        });
        *flow.this.borrow_mut() = Rc::downgrade(&flow);
        flow
    }

    pub fn is_busy(&self) -> bool {
        self.busy.get()
    }

    fn begin(&self) -> u64 {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        self.busy.set(true);
        self.sink.busy(true);
        g
    }

    /// True only for the request that is still the current one.
    fn finish(&self, g: u64) -> bool {
        if self.generation.get() != g || !self.busy.get() {
            return false;
        }
        self.busy.set(false);
        self.sink.busy(false);
        true
    }

    /// Cancel, window closed or superseded: whatever is in flight will do nothing.
    pub fn cancel(&self) {
        let was_busy = self.busy.replace(false);
        self.generation.set(self.generation.get() + 1);
        if was_busy {
            self.backend.cancel();
            self.sink.busy(false);
        }
    }

    /// The one entry point for the dialog and for the address field.
    pub fn connect(&self, raw: &str) {
        let Ok(loc) = parse_smb_input(raw) else {
            self.sink.status(INVALID_ADDRESS);
            return;
        };
        if self.busy.get() {
            return;
        }
        if loc.share.is_none() {
            self.browse(loc);
        } else {
            self.mount(loc);
        }
    }

    /// A share picked from a server chooser.
    pub fn choose(&self, share_uri: &str) {
        match parse_smb_input(share_uri) {
            Ok(loc) if loc.share.is_some() => {
                if !self.busy.get() {
                    self.mount(loc);
                }
            }
            _ => self.sink.status(INVALID_ADDRESS),
        }
    }

    fn browse(&self, loc: SmbLocation) {
        let g = self.begin();
        let weak = self.this.borrow().clone();
        let label = loc.label();
        let host = loc.host.clone();
        self.backend.list_shares(
            &loc.server_uri(),
            Box::new(move |result| {
                let Some(flow) = weak.upgrade() else { return };
                if !flow.finish(g) {
                    return;
                }
                match result {
                    Ok(entries) => flow.sink.shares(&label, share_choices(&host, &entries)),
                    Err(fail) => flow.sink.status(&fail.text(&label)),
                }
            }),
        );
    }

    fn mount(&self, loc: SmbLocation) {
        let Some(share_uri) = loc.share_uri() else { return };
        let g = self.begin();
        let weak = self.this.borrow().clone();
        let label = loc.label();
        let uri = share_uri.clone();
        self.backend.mount_share(
            &share_uri,
            Box::new(move |result| {
                let Some(flow) = weak.upgrade() else { return };
                if flow.generation.get() != g || !flow.busy.get() {
                    return;
                }
                if let Err(fail) = result {
                    if flow.finish(g) {
                        flow.sink.status(&fail.text(&label));
                    }
                    return;
                }
                // Success counts only once the share resolves to one local folder,
                // found off the GTK thread; the request stays busy until then.
                let weak = flow.this.borrow().clone();
                let label = label.clone();
                let loc = loc.clone();
                let subpath = loc.subpath.clone();
                flow.backend.resolve(
                    &uri,
                    &subpath,
                    Box::new(move |opened| {
                        let Some(flow) = weak.upgrade() else { return };
                        if !flow.finish(g) {
                            return;
                        }
                        match opened {
                            Some(path) => flow.sink.opened(&path, &loc),
                            None => flow.sink.status(&SmbFail::Other.text(&label)),
                        }
                    }),
                );
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> SmbLocation {
        parse_smb_input(raw).unwrap_or_else(|e| panic!("{raw:?} should parse, got {e:?}"))
    }

    #[test]
    fn accepted_inputs_converge_on_one_canonical_form_d156_connect() {
        let table: &[(&str, &str, Option<&str>, &[&str])] = &[
            ("smb://NAS", "nas", None, &[]),
            ("smb://nas/", "nas", None, &[]),
            ("SMB://Nas/Media", "nas", Some("Media"), &[]),
            ("smb://nas/Media/Films/2024", "nas", Some("Media"), &["Films", "2024"]),
            ("\\\\NAS", "nas", None, &[]),
            ("\\\\nas\\Media", "nas", Some("Media"), &[]),
            ("\\\\nas\\Media\\Films\\2024\\", "nas", Some("Media"), &["Films", "2024"]),
            ("  smb://nas/My%20Share  ", "nas", Some("My Share"), &[]),
            ("\\\\nas\\My Share", "nas", Some("My Share"), &[]),
            ("smb://192.168.1.5/d", "192.168.1.5", Some("d"), &[]),
            ("smb://nas/M%C3%BCsik", "nas", Some("Müsik"), &[]),
        ];
        for (raw, host, share, sub) in table {
            let loc = ok(raw);
            assert_eq!(loc.host, *host, "{raw}");
            assert_eq!(loc.share.as_deref(), *share, "{raw}");
            assert_eq!(loc.subpath, sub.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{raw}");
        }
        assert_eq!(ok("smb://nas/My Share").share_uri(), Some("smb://nas/My%20Share".to_string()));
        assert_eq!(ok("\\\\nas\\My Share").share_uri(), ok("smb://NAS/My%20Share/").share_uri());
        assert_eq!(ok("smb://nas/Müsik").share_uri(), Some("smb://nas/M%C3%BCsik".to_string()));
        assert_eq!(ok("smb://nas").server_uri(), "smb://nas/");
        assert_eq!(ok("smb://nas/Media").label(), "Media on nas");
        assert_eq!(ok("smb://nas").label(), "nas");
    }

    #[test]
    fn rejected_inputs_fail_before_any_network_call_d156_connect() {
        let table: &[(&str, SmbInputError)] = &[
            ("", SmbInputError::Empty),
            ("   ", SmbInputError::Empty),
            ("nfs://nas/share", SmbInputError::Scheme),
            ("http://nas/share", SmbInputError::Scheme),
            ("/mnt/share", SmbInputError::Scheme),
            ("nas", SmbInputError::Scheme),
            ("smb://", SmbInputError::Host),
            ("\\\\", SmbInputError::Host),
            ("smb:///share", SmbInputError::Host),
            ("smb://nas:445/share", SmbInputError::Host),
            ("smb://user@nas/share", SmbInputError::Credentials),
            ("smb://user:pw@nas/share", SmbInputError::Credentials),
            ("\\\\user@nas\\share", SmbInputError::Credentials),
            ("smb://nas/sha\u{0}re", SmbInputError::Control),
            ("smb://nas/sha\nre", SmbInputError::Control),
            ("smb://nas/%00", SmbInputError::Control),
            ("smb://nas/%zz", SmbInputError::Escape),
            ("smb://nas/%4", SmbInputError::Escape),
            ("smb://nas/%FF", SmbInputError::Escape),
            ("smb://nas/share/..", SmbInputError::Dots),
            ("smb://nas/share/./x", SmbInputError::Dots),
            ("smb://nas/share/%2e%2E/x", SmbInputError::Dots),
            ("\\\\nas\\share\\..\\x", SmbInputError::Dots),
            ("smb://nas/share/a%2Fb", SmbInputError::Malformed),
            ("smb://nas/share//x", SmbInputError::Malformed),
            ("smb://nas/share?x=1", SmbInputError::Malformed),
            ("smb://nas/share#frag", SmbInputError::Malformed),
            ("\\\\nas/share", SmbInputError::Malformed),
        ];
        for (raw, why) in table {
            assert_eq!(parse_smb_input(raw), Err(*why), "{raw:?}");
        }
    }

    fn entries(names: &[&str]) -> Vec<ShareEntry> {
        names.iter().map(|n| ShareEntry::plain(n)).collect()
    }

    #[test]
    fn share_choices_keep_admin_shares_and_are_sorted_and_deduplicated_d156_connect() {
        let got = share_choices("nas", &entries(&["zeta", "Alpha", "alpha", "C$", "IPC$", "", "a/b", "..", "Ba\u{7}d", "My Share", "alpha"]));
        let shown: Vec<(&str, &str)> = got.iter().map(|c| (c.name.as_str(), c.uri.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("Alpha", "smb://nas/Alpha"),
                ("C$", "smb://nas/C%24"),
                ("IPC$", "smb://nas/IPC%24"),
                ("My Share", "smb://nas/My%20Share"),
                ("zeta", "smb://nas/zeta"),
            ],
            "C$ and IPC$ stay; unsafe identities (empty, slash, dots, control) go"
        );
        assert!(share_choices("nas", &[]).is_empty());
    }

    #[test]
    fn share_uri_comes_from_the_actual_name_not_the_display_text_d156_connect() {
        let got = share_choices(
            "nas",
            &[
                ShareEntry { name: "vol1".into(), display: "Family Photos \u{7}".into() },
                ShareEntry { name: "vol1".into(), display: "Other label".into() },
                ShareEntry { name: "x".into(), display: "".into() },
                ShareEntry { name: "docs".into(), display: "user:hunter2@evil/../x".into() },
            ],
        );
        let shown: Vec<(&str, &str)> = got.iter().map(|c| (c.name.as_str(), c.uri.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("docs", "smb://nas/docs"),
                ("Family Photos", "smb://nas/vol1"),
                ("x", "smb://nas/x"),
            ]
        );
    }

    #[test]
    fn over_cap_shares_are_cut_after_sorting_so_the_kept_set_is_stable_d156_connect() {
        let mut names: Vec<String> = (0..MAX_SHARES + 120).map(|i| format!("share{i:04}")).collect();
        let forward = share_choices("nas", &names.iter().map(|n| ShareEntry::plain(n)).collect::<Vec<_>>());
        // a deterministic shuffle: stride permutation
        let n = names.len();
        names = (0..n).map(|i| names[(i * 7 + 3) % n].clone()).collect();
        let shuffled = share_choices("nas", &names.iter().map(|n| ShareEntry::plain(n)).collect::<Vec<_>>());
        assert_eq!(forward.len(), MAX_SHARES);
        assert_eq!(forward, shuffled);
        assert_eq!(forward[0].uri, "smb://nas/share0000");
        assert_eq!(forward[MAX_SHARES - 1].uri, format!("smb://nas/share{:04}", MAX_SHARES - 1));
    }

    fn mounted(host: &str, share: &str, path: &Path) -> MountedShare {
        MountedShare { host: host.into(), share: share.into(), path: path.to_path_buf() }
    }

    #[test]
    fn root_lookup_is_by_name_only_and_exact_d156_connect() {
        let loc = ok("smb://NAS/Media");
        let none = || Vec::new();
        // a native absolute path is taken as is, even if it does not exist: no metadata call
        let ghost = PathBuf::from("/nonexistent/wf-ghost");
        assert_eq!(lookup_root(Some(&ghost), &none, &loc), Lookup::Found(ghost.clone()));
        let one = || vec![mounted("nas", "media", Path::new("/g/a"))];
        assert_eq!(lookup_root(Some(Path::new("rel")), &one, &loc), Lookup::Found("/g/a".into()));
        assert_eq!(lookup_root(None, &one, &loc), Lookup::Found("/g/a".into()));
        assert_eq!(lookup_root(None, &none, &loc), Lookup::NotYet);
        let twice = || vec![mounted("Nas", "MEDIA", Path::new("/g/a")), mounted("nas", "media", Path::new("/g/a"))];
        assert_eq!(lookup_root(None, &twice, &loc), Lookup::Found("/g/a".into()));
        let two = || vec![mounted("nas", "media", Path::new("/g/a")), mounted("nas", "media", Path::new("/g/b"))];
        assert_eq!(lookup_root(None, &two, &loc), Lookup::Fail, "ambiguous");
        let decoys = || {
            vec![
                mounted("nas2", "media", Path::new("/g/a")),
                mounted("nas", "media2", Path::new("/g/a")),
                mounted("nas", "medi", Path::new("/g/a")),
                mounted("nas", "media", Path::new("relative")),
            ]
        };
        assert_eq!(lookup_root(None, &decoys, &loc), Lookup::NotYet);
    }

    #[test]
    fn subpath_lookup_stays_under_the_root_and_share_only_does_no_fs_work_d156_connect() {
        let root = std::env::temp_dir().join(format!("wf-smb-sub-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("wf-smb-out-{}", std::process::id()));
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
        std::fs::create_dir_all(root.join("Films")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let none = || Vec::new();
        assert_eq!(lookup_full(Some(&root), &none, &ok("smb://nas/Media/Films")), Lookup::Found(root.join("Films")));
        assert_eq!(lookup_full(Some(&root), &none, &ok("smb://nas/Media/escape")), Lookup::Fail, "symlink escape");
        assert_eq!(lookup_full(Some(&root), &none, &ok("smb://nas/Media/missing")), Lookup::Fail);
        let ghost = PathBuf::from("/nonexistent/wf-ghost");
        assert_eq!(lookup_full(Some(&ghost), &none, &ok("smb://nas/Media")), Lookup::Found(ghost), "share root: no fs work");
        for d in [&root, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// A manual scheduler: nothing runs until the test says so.
    #[derive(Default)]
    struct Manual {
        jobs: RefCell<Vec<(Box<dyn FnOnce() -> Lookup + Send>, Box<dyn FnOnce(Lookup)>)>>,
        timers: RefCell<Vec<(u32, Box<dyn FnOnce()>)>>,
    }
    impl Sched for Manual {
        fn work(&self, job: Box<dyn FnOnce() -> Lookup + Send>, then: Box<dyn FnOnce(Lookup)>) {
            self.jobs.borrow_mut().push((job, then));
        }
        fn after(&self, millis: u32, then: Box<dyn FnOnce()>) {
            self.timers.borrow_mut().push((millis, then));
        }
    }
    impl Manual {
        fn run_job(&self) -> bool {
            let next = {
                let mut jobs = self.jobs.borrow_mut();
                (!jobs.is_empty()).then(|| jobs.remove(0))
            };
            let Some((job, then)) = next else { return false };
            then(job());
            true
        }
        fn fire_timer(&self) -> Option<u32> {
            let next = {
                let mut timers = self.timers.borrow_mut();
                (!timers.is_empty()).then(|| timers.remove(0))
            };
            let (ms, then) = next?;
            then();
            Some(ms)
        }
    }

    fn driver(script: Vec<Lookup>) -> (Rc<Manual>, Rc<Cell<bool>>, Rc<RefCell<Vec<Option<PathBuf>>>>, Arc<std::sync::atomic::AtomicUsize>) {
        let sched = Rc::new(Manual::default());
        let alive = Rc::new(Cell::new(true));
        let out = Rc::new(RefCell::new(Vec::new()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let script = Arc::new(std::sync::Mutex::new(script));
        let lookup: Arc<dyn Fn() -> Lookup + Send + Sync> = {
            let (script, calls) = (script.clone(), calls.clone());
            Arc::new(move || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut s = script.lock().unwrap();
                if s.len() > 1 { s.remove(0) } else { s[0].clone() }
            })
        };
        let alive_fn = { let a = alive.clone(); Rc::new(move || a.get()) as Rc<dyn Fn() -> bool> };
        let o = out.clone();
        resolve_async(sched.clone(), alive_fn, lookup, Box::new(move |r| o.borrow_mut().push(r)));
        (sched, alive, out, calls)
    }

    #[test]
    fn resolution_waits_off_thread_retries_a_bounded_number_of_times_d156_connect() {
        let found = || Lookup::Found("/g/a".into());
        // immediate success: nothing has run until the worker is driven
        let (sched, _alive, out, calls) = driver(vec![found()]);
        assert!(out.borrow().is_empty(), "nothing on the calling thread");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0, "no lookup on the calling thread");
        assert!(sched.run_job());
        assert_eq!(*out.borrow(), [Some(PathBuf::from("/g/a"))]);
        // delayed publication: two misses, then found, on timers
        let (sched, _a, out, calls) = driver(vec![Lookup::NotYet, Lookup::NotYet, found()]);
        assert!(sched.run_job());
        assert_eq!(sched.fire_timer(), Some(RESOLVE_RETRY_MS));
        assert!(sched.run_job());
        assert!(out.borrow().is_empty());
        assert_eq!(sched.fire_timer(), Some(RESOLVE_RETRY_MS));
        assert!(sched.run_job());
        assert_eq!(*out.borrow(), [Some(PathBuf::from("/g/a"))]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        // exhausted: fails exactly once after the bound
        let (sched, _a, out, calls) = driver(vec![Lookup::NotYet]);
        while sched.run_job() {
            sched.fire_timer();
        }
        assert_eq!(*out.borrow(), [None]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst) as u32, RESOLVE_ATTEMPTS);
        // ambiguous is final at once
        let (sched, _a, out, calls) = driver(vec![Lookup::Fail]);
        assert!(sched.run_job());
        assert_eq!((out.borrow().clone(), calls.load(std::sync::atomic::Ordering::SeqCst)), (vec![None], 1));
        assert!(sched.timers.borrow().is_empty());
    }

    #[test]
    fn cancelling_stops_a_running_worker_and_a_scheduled_retry_d156_connect() {
        // cancelled while the worker is in flight
        let (sched, alive, out, _c) = driver(vec![Lookup::Found("/g/a".into())]);
        alive.set(false);
        sched.run_job();
        assert!(out.borrow().is_empty(), "a late worker result does nothing");
        // cancelled while a retry timer is pending
        let (sched, alive, out, calls) = driver(vec![Lookup::NotYet]);
        sched.run_job();
        alive.set(false);
        sched.fire_timer();
        assert!(!sched.run_job(), "no further worker after cancel");
        assert!(out.borrow().is_empty());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn a_credential_shaped_display_name_falls_back_to_the_safe_identity_d156_connect() {
        let got = share_choices(
            "nas",
            &[
                ShareEntry { name: "vol1".into(), display: "smb://user:hunter2@evil/x".into() },
                ShareEntry { name: "vol2".into(), display: "bob@host".into() },
                ShareEntry { name: "vol3".into(), display: "my password is x".into() },
            ],
        );
        let shown: Vec<(&str, &str)> = got.iter().map(|c| (c.name.as_str(), c.uri.as_str())).collect();
        assert_eq!(shown, [("vol1", "smb://nas/vol1"), ("vol2", "smb://nas/vol2"), ("vol3", "smb://nas/vol3")]);
    }

    #[test]
    fn hostile_stored_labels_are_never_used_d156_connect() {
        let hostile = "user:hunter2@nas\u{7}\n".to_string() + &"x".repeat(5000);
        let entry = RememberedNetwork { uri: "smb://nas/Media".into(), label: hostile.clone() };
        assert_eq!(remembered_label(&entry).as_deref(), Some("Media on nas"));
        let bad = RememberedNetwork { uri: "smb://user@nas/Media".into(), label: hostile.clone() };
        assert_eq!(remembered_label(&bad), None);
        let mut list = vec![entry, bad.clone()];
        assert!(remember(&mut list, &ok("smb://box/Docs")));
        assert!(forget(&mut list, "smb://box/Docs"));
        assert_eq!(list[0].label, "Media on nas", "a valid entry is rewritten from its URI");
        assert_eq!(list[1], bad, "an invalid entry is left alone, not rewritten");
    }

    #[test]
    fn remembering_dedups_forms_forgets_one_and_drops_unsafe_entries_d156_connect() {
        let mut list = Vec::new();
        assert!(remember(&mut list, &ok("smb://nas/Media")));
        assert!(remember(&mut list, &ok("\\\\NAS\\media\\Films")), "UNC form with a subpath is the same share");
        assert_eq!(list.len(), 1, "equivalent forms are one entry");
        assert!(remember(&mut list, &ok("smb://box/Docs")));
        assert_eq!(list.iter().map(|e| e.uri.as_str()).collect::<Vec<_>>(), ["smb://box/Docs", "smb://nas/media"]);
        assert_eq!(list[0].label, "Docs on box");
        assert!(!remember(&mut list, &ok("smb://nas")), "a server alone is never remembered");
        assert!(!forget(&mut list, "smb://nowhere/x"));
        assert!(forget(&mut list, "smb://NAS/Media"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].uri, "smb://box/Docs", "Forget removes only its target");
        for i in 0..30 {
            remember(&mut list, &ok(&format!("smb://h{i}/s")));
        }
        assert_eq!(list.len(), MAX_REMEMBERED);
        let bad = |uri: &str| RememberedNetwork { uri: uri.into(), label: "x".into() };
        assert!(remembered_location(&bad("smb://nas/Media")).is_some());
        assert!(remembered_location(&bad("smb://user@nas/Media")).is_none());
        assert!(remembered_location(&bad("smb://nas/Media/sub")).is_none());
        assert!(remembered_location(&bad("smb://nas")).is_none());
        assert!(remembered_location(&bad("/run/user/1000/gvfs/smb-share:server=nas,share=media")).is_none());
        assert!(remembered_location(&bad("smb://NAS/Media")).is_none(), "only the canonical spelling is trusted");
    }

    #[test]
    fn descend_stays_under_the_mounted_root_d156_connect() {
        let root = crate::native_core::scratch_root().join(format!("wf-smb-descend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let outside = root.join("outside");
        let share = root.join("share");
        std::fs::create_dir_all(share.join("Films/2024")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(share.join("file.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, share.join("escape")).unwrap();
        let sub = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(descend(&share, &[]), Some(share.clone()));
        assert_eq!(descend(&share, &sub(&["Films", "2024"])), Some(share.join("Films/2024")));
        assert_eq!(descend(&share, &sub(&["missing"])), None);
        assert_eq!(descend(&share, &sub(&["file.txt"])), None, "a file is not a folder");
        assert_eq!(descend(&share, &sub(&["escape"])), None, "a symlink out of the share is refused");
        assert_eq!(descend(&share.join("nope"), &[]), None);
        assert_eq!(descend(Path::new("relative"), &[]), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- the flow, against a fake backend ------------------------------------

    #[derive(Default)]
    struct FakeState {
        listed: Vec<String>,
        mounted: Vec<String>,
        cancels: usize,
        pending_list: Option<Box<dyn FnOnce(Result<Vec<ShareEntry>, SmbFail>)>>,
        pending_mount: Option<Box<dyn FnOnce(Result<(), SmbFail>)>>,
        hold: bool,
        list_result: Option<Result<Vec<String>, SmbFail>>,
        mount_result: Option<Result<(), SmbFail>>,
        local: Option<PathBuf>,
        local_asked: Vec<String>,
        hold_resolve: bool,
        pending_resolve: Option<Box<dyn FnOnce(Option<PathBuf>)>>,
    }

    #[derive(Default)]
    struct Fake(RefCell<FakeState>);

    impl SmbBackend for Fake {
        fn list_shares(&self, uri: &str, done: Box<dyn FnOnce(Result<Vec<ShareEntry>, SmbFail>)>) {
            let mut s = self.0.borrow_mut();
            s.listed.push(uri.to_string());
            if s.hold {
                s.pending_list = Some(done);
                return;
            }
            let r = s.list_result.clone().unwrap_or(Ok(Vec::new())).map(|v| v.iter().map(|n| ShareEntry::plain(n)).collect());
            drop(s);
            done(r);
        }
        fn mount_share(&self, uri: &str, done: Box<dyn FnOnce(Result<(), SmbFail>)>) {
            let mut s = self.0.borrow_mut();
            s.mounted.push(uri.to_string());
            if s.hold {
                s.pending_mount = Some(done);
                return;
            }
            let r = s.mount_result.unwrap_or(Ok(()));
            drop(s);
            done(r);
        }
        fn resolve(&self, uri: &str, subpath: &[String], done: Box<dyn FnOnce(Option<PathBuf>)>) {
            let mut s = self.0.borrow_mut();
            s.local_asked.push(uri.to_string());
            if s.hold_resolve {
                s.pending_resolve = Some(done);
                return;
            }
            let found = s.local.clone().and_then(|root| if subpath.is_empty() { Some(root) } else { descend(&root, subpath) });
            drop(s);
            done(found);
        }
        fn cancel(&self) {
            self.0.borrow_mut().cancels += 1;
        }
    }

    #[derive(Default)]
    struct Recorder {
        log: RefCell<Vec<String>>,
        opened: RefCell<Vec<(PathBuf, String)>>,
        choices: RefCell<Vec<Vec<ShareChoice>>>,
    }

    impl SmbSink for Recorder {
        fn busy(&self, on: bool) {
            self.log.borrow_mut().push(format!("busy:{on}"));
        }
        fn status(&self, text: &str) {
            self.log.borrow_mut().push(format!("status:{text}"));
        }
        fn shares(&self, server: &str, choices: Vec<ShareChoice>) {
            self.log.borrow_mut().push(format!("shares:{server}:{}", choices.len()));
            self.choices.borrow_mut().push(choices);
        }
        fn opened(&self, local: &Path, share: &SmbLocation) {
            self.opened.borrow_mut().push((local.to_path_buf(), share.share_uri().unwrap()));
        }
    }

    fn rig() -> (Rc<SmbFlow>, Rc<Fake>, Rc<Recorder>, PathBuf) {
        let root = crate::native_core::scratch_root()
            .join(format!("wf-smb-flow-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Films")).unwrap();
        let fake = Rc::new(Fake::default());
        fake.0.borrow_mut().local = Some(root.clone());
        let rec = Rc::new(Recorder::default());
        (SmbFlow::new(fake.clone(), rec.clone()), fake, rec, root)
    }

    #[test]
    fn both_spellings_converge_and_a_direct_share_skips_browse_d156_connect() {
        for raw in ["smb://nas/Media/Films", "\\\\NAS\\Media\\Films"] {
            let (flow, fake, rec, root) = rig();
            flow.connect(raw);
            let f = fake.0.borrow();
            assert!(f.listed.is_empty(), "a share address is never browsed");
            assert_eq!(f.mounted, ["smb://nas/Media"]);
            assert_eq!(f.local_asked, ["smb://nas/Media"], "success is verified by resolving the local path");
            drop(f);
            assert_eq!(*rec.opened.borrow(), [(root.join("Films"), "smb://nas/Media".to_string())], "{raw}");
            assert!(!flow.is_busy());
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn a_server_address_lists_shares_and_choosing_one_mounts_it_d156_connect() {
        let (flow, fake, rec, root) = rig();
        fake.0.borrow_mut().list_result =
            Some(Ok(vec!["Media".into(), "IPC$".into(), "Docs".into(), "Media".into()]));
        flow.connect("\\\\NAS");
        assert_eq!(fake.0.borrow().listed, ["smb://nas/"]);
        assert!(fake.0.borrow().mounted.is_empty(), "nothing is mounted before a choice");
        let choices = rec.choices.borrow()[0].clone();
        assert_eq!(choices.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Docs", "IPC$", "Media"]);
        assert!(rec.opened.borrow().is_empty());
        flow.choose(&choices[2].uri);
        assert_eq!(fake.0.borrow().mounted, ["smb://nas/Media"]);
        assert_eq!(*rec.opened.borrow(), [(root.clone(), "smb://nas/Media".to_string())]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn browse_keeps_the_chooser_usable_for_empty_denied_and_partial_results_d156_connect() {
        let (flow, fake, rec, root) = rig();
        flow.connect("smb://nas");
        assert!(rec.log.borrow().contains(&"shares:nas:0".to_string()), "empty list is a usable chooser");
        fake.0.borrow_mut().list_result = Some(Err(SmbFail::Denied));
        flow.connect("smb://nas");
        assert!(rec.log.borrow().contains(&"status:Access to nas was denied.".to_string()));
        fake.0.borrow_mut().list_result = Some(Ok(vec!["Only".into()]));
        flow.connect("smb://nas");
        assert!(rec.log.borrow().contains(&"shares:nas:1".to_string()), "a partial list still shows");
        assert!(!flow.is_busy() && rec.opened.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failures_never_navigate_and_never_echo_backend_or_raw_text_d156_connect() {
        for fail in [SmbFail::Cancelled, SmbFail::Denied, SmbFail::Unreachable, SmbFail::NotSupported, SmbFail::Other] {
            let (flow, fake, rec, root) = rig();
            fake.0.borrow_mut().mount_result = Some(Err(fail));
            flow.connect("smb://nas/Media");
            assert!(rec.opened.borrow().is_empty());
            assert!(fake.0.borrow().local_asked.is_empty(), "no local path is asked for after a failure");
            let log = rec.log.borrow().join("|");
            assert!(log.contains(&fail.text("Media on nas")), "{fail:?}: {log}");
            assert!(!flow.is_busy());
            let _ = std::fs::remove_dir_all(&root);
        }
        // Missing local path, a missing subfolder and a symlink escape all refuse.
        for (raw, local_ok) in [("smb://nas/Media", false), ("smb://nas/Media/Absent", true), ("smb://nas/Media/x/..", true)] {
            let (flow, fake, rec, root) = rig();
            if !local_ok {
                fake.0.borrow_mut().local = None;
            }
            flow.connect(raw);
            assert!(rec.opened.borrow().is_empty(), "{raw}");
            assert!(!flow.is_busy(), "{raw}");
            let _ = std::fs::remove_dir_all(&root);
        }
        // Invalid or credential-bearing input is refused with the fixed text and no backend call.
        let (flow, fake, rec, root) = rig();
        for raw in ["smb://user:hunter2@nas/Media", "nfs://nas/x", "smb://nas/%zz", ""] {
            flow.connect(raw);
        }
        let f = fake.0.borrow();
        assert!(f.listed.is_empty() && f.mounted.is_empty());
        let log = rec.log.borrow().join("|");
        assert!(!log.contains("hunter2") && !log.contains("user"), "rejected raw input is never echoed: {log}");
        assert_eq!(rec.log.borrow().iter().filter(|l| l.as_str() == format!("status:{INVALID_ADDRESS}")).count(), 4);
        drop(f);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn repeated_connects_do_not_overlap_and_late_results_do_nothing_d156_connect() {
        let (flow, fake, rec, root) = rig();
        fake.0.borrow_mut().hold = true;
        flow.connect("smb://nas/Media");
        flow.connect("smb://nas/Media");
        flow.choose("smb://nas/Media");
        assert_eq!(fake.0.borrow().mounted.len(), 1, "repeated Enter or clicks while busy are ignored");
        assert!(flow.is_busy());
        // Cancel, then the late success arrives: nothing happens.
        flow.cancel();
        assert_eq!(fake.0.borrow().cancels, 1);
        assert!(!flow.is_busy());
        let late = fake.0.borrow_mut().pending_mount.take().unwrap();
        late(Ok(()));
        assert!(rec.opened.borrow().is_empty(), "a cancelled request never navigates");
        assert!(fake.0.borrow().local_asked.is_empty());
        // A superseding request: the old callback is stale even if it arrives first.
        flow.connect("smb://nas/Media");
        let stale = fake.0.borrow_mut().pending_mount.take().unwrap();
        flow.cancel();
        flow.connect("smb://box/Docs");
        stale(Ok(()));
        assert!(rec.opened.borrow().is_empty() && flow.is_busy(), "the stale result neither opens nor unbusies");
        let current = fake.0.borrow_mut().pending_mount.take().unwrap();
        current(Ok(()));
        assert_eq!(rec.opened.borrow().len(), 1);
        assert_eq!(rec.opened.borrow()[0].1, "smb://box/Docs");
        // A callback that outlives the flow does nothing.
        fake.0.borrow_mut().hold = true;
        flow.connect("smb://nas/Media");
        let orphan = fake.0.borrow_mut().pending_mount.take().unwrap();
        drop(flow);
        orphan(Ok(()));
        assert_eq!(rec.opened.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pending_resolution_keeps_the_request_busy_and_late_results_are_inert_d156_connect() {
        let (flow, fake, rec, root) = rig();
        fake.0.borrow_mut().hold_resolve = true;
        flow.connect("smb://nas/Media");
        assert!(flow.is_busy(), "still busy while the path is being resolved");
        assert!(rec.opened.borrow().is_empty());
        assert_eq!(fake.0.borrow().mounted.len(), 1);
        flow.connect("smb://nas/Media");
        assert_eq!(fake.0.borrow().mounted.len(), 1, "no second attempt while pending");
        // cancel: the late resolution does nothing
        flow.cancel();
        let late = fake.0.borrow_mut().pending_resolve.take().unwrap();
        late(Some(root.clone()));
        assert!(rec.opened.borrow().is_empty());
        assert!(!flow.is_busy());
        // a superseding request: only the newest resolution opens
        flow.connect("smb://nas/Media");
        let stale = fake.0.borrow_mut().pending_resolve.take().unwrap();
        flow.cancel();
        flow.connect("smb://nas/Media");
        let current = fake.0.borrow_mut().pending_resolve.take().unwrap();
        stale(Some(root.clone()));
        assert!(rec.opened.borrow().is_empty());
        current(Some(root.clone()));
        assert_eq!(rec.opened.borrow().len(), 1);
        // unresolved: one plain status, nothing opened
        fake.0.borrow_mut().hold_resolve = false;
        fake.0.borrow_mut().local = None;
        let before = rec.opened.borrow().len();
        flow.connect("smb://nas/Media");
        assert_eq!(rec.opened.borrow().len(), before);
        assert!(rec.log.borrow().last().unwrap().starts_with("status:"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_credential_shaped_text_reaches_the_sink_or_a_saved_entry_d156_connect() {
        let (flow, _fake, rec, root) = rig();
        flow.connect("smb://nas/Media");
        let mut list = Vec::new();
        let (_, uri) = rec.opened.borrow()[0].clone();
        remember(&mut list, &parse_smb_input(&uri).unwrap());
        let saved = format!("{} {}", list[0].uri.replace("smb://", ""), list[0].label);
        for bad in ["@", ":", "password", "/run/user", "gvfs", "?", "#"] {
            assert!(!saved.contains(bad), "{bad} in {saved}");
        }
        assert!(!rec.log.borrow().join("|").contains("gvfs"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
