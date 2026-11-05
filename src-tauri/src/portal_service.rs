// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 LegendofZito (Kadaken)

//! `org.freedesktop.impl.portal.FileChooser` backend contract for
//! Wayfinder. Exports `OpenFile` and `SaveFile`; `SaveFiles` (a whole
//! directory of files in one request) is not exported yet.
//!
//! Exporting an interface means claiming ALL of it: a registration that
//! names FileChooser and then answers only `OpenFile` leaves every Save
//! request in the session unanswered, and the app that asked shows no
//! dialog at all (Chrome's "Save image as" simply did nothing). That is
//! why `SaveFile` is part of the same interface impl rather than a later
//! stage. This file owns all service logic; `native.rs` must
//! never import it or call anything here, and this file must never
//! initialize GTK, GStreamer or a display.

use adw::gtk::gio;
use adw::prelude::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

/// One supplied filter's user-visible name and ordered glob/MIME rules.
/// `kind` is `0` for a glob pattern, `1` for a MIME type (matching the
/// real `org.freedesktop.portal.FileChooser` `filters` wire shape
/// `a(sa(us))`); wire-identical in `native.rs`, which defines its own copy
/// rather than importing this file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireFilter {
    pub name: String,
    pub rules: Vec<(u32, String)>,
}

/// One supplied choice: an empty `options` list is the contract's boolean
/// choice: a nonempty list is enumerated. `initial` is always a concrete,
/// already-resolved valid value by the time this reaches the native picker
/// -- never empty, never an id absent from `options` for an enumerated
/// choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireChoice {
    pub id: String,
    pub label: String,
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// The one per-request protocol file this round adds: written by the
/// portal into the already-owned `RequestRoot` before the native child is
/// spawned, read by `wayfinder-native` via `--request <path>`. Carries
/// exactly the decoded, already-resolved request data; never an env var,
/// never a global file, never a caller-chosen path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireRequest {
    pub multiple: bool,
    pub filters: Vec<WireFilter>,
    pub current_filter: Option<WireFilter>,
    pub choices: Vec<WireChoice>,
    /// True for a `SaveFile` request: the picker names a file to write
    /// rather than choosing one that already exists.
    #[serde(default)]
    pub save: bool,
    /// The file name the app suggests, already validated as a plain name
    /// (no separator, no NUL, never "." or ".."). Empty when none was
    /// supplied.
    #[serde(default)]
    pub current_name: String,
    /// The folder the app suggests opening in, absolute. `None` when none
    /// was supplied or it was not usable.
    #[serde(default)]
    pub current_folder: Option<String>,
    /// An existing file the app suggests replacing, absolute.
    #[serde(default)]
    pub current_file: Option<String>,
}

impl Default for WireRequest {
    fn default() -> Self {
        Self {
            multiple: false,
            filters: Vec::new(),
            current_filter: None,
            choices: Vec::new(),
            save: false,
            current_name: String::new(),
            current_folder: None,
            current_file: None,
        }
    }
}

/// The native picker's response metadata beyond the picked paths
/// themselves: the filter actually active, and every choice's final
/// `(id, selected_id)` pair. Read from a second, separately-optional
/// `result.meta.json` file in the same `RequestRoot`; the existing
/// strict path-only `read_result_strict` parser is untouched by this.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireResultMeta {
    #[serde(default)]
    pub current_filter: Option<WireFilter>,
    #[serde(default)]
    pub choices: Vec<(String, String)>,
}

pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.wayfinder";
pub const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
pub const INTERFACE_NAME: &str = "org.freedesktop.impl.portal.FileChooser";

const RESPONSE_SUCCESS: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;
const RESPONSE_OTHER: u32 = 2;

/// The one injected single-selection chooser seam. `Picked` carries
/// whatever the chooser actually produced -- including an illegal
/// cardinality or an unnormalizable path -- so the service's own
/// result-decision guards have something real to refuse; the seam
/// itself never pre-filters that for the service.
#[derive(Debug, Clone)]
pub enum ChooserOutcome {
    Picked(Vec<PathBuf>),
    Cancelled,
    Failed,
}

pub trait Chooser: Send + Sync {
    fn pick_single(&self, app_id: &str, parent_window: &str, title: &str) -> ChooserOutcome;

    /// Additive seam for D-222: carries the decoded `multiple`/`filters`/
    /// `current_filter`/`choices` request to the chooser. The default body
    /// delegates to `pick_single` and reports no filter/choice metadata, so
    /// every existing `Chooser` impl and every existing `pick_single` call
    /// site in this file's own tests is unaffected; only `ProcessChooser`
    /// overrides it with real behavior, and only `decide()`'s own call site
    /// uses it.
    fn pick(&self, app_id: &str, parent_window: &str, title: &str, request: &WireRequest) -> PickOutcome {
        let _ = request;
        PickOutcome::from(self.pick_single(app_id, parent_window, title))
    }
}

/// `ChooserOutcome` plus whatever filter/choice metadata the chooser
/// resolved, if any. `From<ChooserOutcome>` gives it empty metadata, so
/// every pre-existing direct `decide()` test (which builds a bare
/// `ChooserOutcome`) only has to add `.into()` at its one closure call site.
#[derive(Debug, Clone)]
pub struct PickOutcome {
    pub outcome: ChooserOutcome,
    pub meta: WireResultMeta,
}

impl From<ChooserOutcome> for PickOutcome {
    fn from(outcome: ChooserOutcome) -> Self {
        Self {
            outcome,
            meta: WireResultMeta::default(),
        }
    }
}

/// A fresh, owner-only (mode 0700) directory holding exactly one
/// request's result file, never under `/tmp`. Removed on every exit
/// path: `Drop` always makes one unconditional best-effort attempt, so
/// production cleanup failure can never panic the portal daemon; a
/// dedicated unit test (not this type itself) is what makes a normal-path
/// cleanup failure observable.
struct RequestRoot {
    dir: PathBuf,
}

impl RequestRoot {
    fn create_under(base: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(base)?;
        fs::set_permissions(base, fs::Permissions::from_mode(0o700))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("req-{stamp:x}-{:x}", std::process::id()));
        fs::create_dir(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir })
    }

    fn result_path(&self) -> PathBuf {
        self.dir.join("result")
    }

    fn request_path(&self) -> PathBuf {
        self.dir.join("request.json")
    }

    fn result_meta_path(&self) -> PathBuf {
        self.dir.join("result.meta.json")
    }
}

/// The largest request/result-meta file either side will write or read.
/// Same bound as `MAX_RESULT_BYTES`; a request this large is malformed.
const MAX_WIRE_JSON_BYTES: u64 = 64 * 1024;

/// Writes the decoded request as owner-only (mode 0600) JSON so
/// `wayfinder-native` can read it via `--request <path>`. Never partial:
/// any serialization or I/O failure removes nothing written so far and
/// is reported to the caller, which fails the whole pick closed.
fn write_request_file(path: &Path, request: &WireRequest) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(request)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if bytes.len() as u64 > MAX_WIRE_JSON_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request exceeds the wire size limit",
        ));
    }
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Reads the optional `result.meta.json` the native picker may have
/// written. Every failure mode -- absent, oversized, unreadable, not
/// valid JSON, wrong shape -- returns an empty default rather than
/// erroring; this file is advisory metadata, never load-bearing for
/// whether the pick itself succeeded.
fn read_result_meta(path: &Path) -> WireResultMeta {
    let Ok(metadata) = fs::metadata(path) else {
        return WireResultMeta::default();
    };
    if metadata.len() > MAX_WIRE_JSON_BYTES {
        return WireResultMeta::default();
    }
    let Ok(bytes) = fs::read(path) else {
        return WireResultMeta::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

impl Drop for RequestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Stage-two production chooser: launches the sibling `wayfinder-native`
/// binary as `--pick --out <owned-result-path>`, never in-process, and
/// never importing or initializing GTK/GStreamer/a display here. The
/// three fields below are the only test-only decision seams -- plain
/// constructor arguments, never environment variables or anything a real
/// D-Bus caller can reach.
pub struct ProcessChooser {
    executable: PathBuf,
    deadline: Duration,
    /// `None` means the owner-only picker root could not be derived (the
    /// production case: `XDG_RUNTIME_DIR` absent). Every pick then fails
    /// closed without ever spawning a child -- never a daemon panic.
    scratch_base: Option<PathBuf>,
    /// Exists only in test builds (`cfg(test)` is crate-wide during
    /// `cargo test`), defaults to every guard active, and is never set
    /// to anything else outside `#[cfg(test)]` test code -- there is no
    /// field, flag or environment variable reachable from `production()`
    /// or a real D-Bus caller that can touch it. Read only through the
    /// `should_*`/`exit_classification_override` accessors below, whose
    /// `cfg(not(test))` twin is a hardcoded, zero-cost constant, so the
    /// shipped binary carries no branch capable of reaching this at all.
    #[cfg(test)]
    test_override: TestOverride,
}

/// Named, narrow test-only decision-bypass seams, one per production
/// guard, each defaulting to the guard being fully active. A deletion-
/// oracle test flips exactly one of these against otherwise-identical
/// evidence (same executable, same exit code, same result bytes) and
/// shows the *production* outcome turns wrong only because that one
/// guard was removed -- proving the guard, not the surrounding harness,
/// is load-bearing.
#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct TestOverride {
    ignore_deadline: bool,
    accept_any_exit_code: bool,
    ignore_result_shape: bool,
    skip_cleanup: bool,
    treat_spawn_error_as_success: bool,
}

/// The largest result file `ProcessChooser` will read. Bounds the
/// allocation before any byte is trusted; anything over this is
/// malformed, never partially parsed.
const MAX_RESULT_BYTES: u64 = 64 * 1024;

/// The exact, narrow shape the native `--pick --out` contract promises:
/// zero bytes for a confirmed Cancel, or one-or-more nonempty,
/// newline-terminated path records for a successful pick. Nothing else
/// -- no blank record, no missing terminator, no partial line -- is a
/// valid instance of either variant.
enum ParsedResult {
    EmptyCancel,
    Paths(Vec<PathBuf>),
}

/// Reads and validates the result file against the exact protocol
/// shape. `None` covers every malformed case: absent, unreadable,
/// oversized, not valid UTF-8, missing trailing newline, or containing
/// a blank record.
fn read_result_strict(result_path: &Path) -> Option<ParsedResult> {
    let mut file = fs::File::open(result_path).ok()?;
    let mut probe = [0u8; 1];
    let mut buf = Vec::new();
    loop {
        if (buf.len() as u64) > MAX_RESULT_BYTES {
            return None;
        }
        let n = file.read(&mut probe).ok()?;
        if n == 0 {
            break;
        }
        buf.push(probe[0]);
    }
    if buf.is_empty() {
        return Some(ParsedResult::EmptyCancel);
    }
    let text = std::str::from_utf8(&buf).ok()?;
    if !text.ends_with('\n') {
        return None;
    }
    let body = &text[..text.len() - 1];
    if body.is_empty() {
        return None;
    }
    let mut paths = Vec::new();
    for line in body.split('\n') {
        if line.is_empty() {
            return None;
        }
        paths.push(PathBuf::from(line));
    }
    Some(ParsedResult::Paths(paths))
}

impl ProcessChooser {
    pub fn production() -> Self {
        let executable = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|dir| dir.join("wayfinder-native")))
            .unwrap_or_else(|| PathBuf::from("wayfinder-native"));
        Self {
            executable,
            deadline: Duration::from_secs(120),
            scratch_base: production_scratch_base(),
            #[cfg(test)]
            test_override: TestOverride::default(),
        }
    }

    #[cfg(test)]
    fn for_test(executable: PathBuf, deadline: Duration, scratch_base: PathBuf) -> Self {
        Self {
            executable,
            deadline,
            scratch_base: Some(scratch_base),
            test_override: TestOverride::default(),
        }
    }

    #[cfg(test)]
    fn for_test_with_override(
        executable: PathBuf,
        deadline: Duration,
        scratch_base: PathBuf,
        test_override: TestOverride,
    ) -> Self {
        Self {
            executable,
            deadline,
            scratch_base: Some(scratch_base),
            test_override,
        }
    }

    #[cfg(test)]
    fn should_ignore_deadline(&self) -> bool {
        self.test_override.ignore_deadline
    }
    #[cfg(not(test))]
    fn should_ignore_deadline(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn should_accept_any_exit_code(&self) -> bool {
        self.test_override.accept_any_exit_code
    }
    #[cfg(not(test))]
    fn should_accept_any_exit_code(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn should_ignore_result_shape(&self) -> bool {
        self.test_override.ignore_result_shape
    }
    #[cfg(not(test))]
    fn should_ignore_result_shape(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn should_skip_cleanup(&self) -> bool {
        self.test_override.skip_cleanup
    }
    #[cfg(not(test))]
    fn should_skip_cleanup(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn should_treat_spawn_error_as_success(&self) -> bool {
        self.test_override.treat_spawn_error_as_success
    }
    #[cfg(not(test))]
    fn should_treat_spawn_error_as_success(&self) -> bool {
        false
    }

    /// Waits for the child, polling with a bound. On timeout or a
    /// parent-side wait error, kills and reaps the *exact owned process
    /// group* the child was spawned into (never a reused PID, never an
    /// unrelated process) so a descendant the child spawned -- e.g. a
    /// shell's own child -- cannot outlive it as an orphan.
    fn wait_with_deadline(&self, mut child: Child) -> Option<std::process::ExitStatus> {
        let pgid = child.id() as libc::pid_t;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {
                    if !self.should_ignore_deadline() && start.elapsed() >= self.deadline {
                        kill_owned_process_group(pgid);
                        let _ = child.wait();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => {
                    kill_owned_process_group(pgid);
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
}

/// Sends `SIGKILL` to the exact process group the owned child was
/// placed into at spawn time (negative pid), then to the child's own
/// pid as a bounded best-effort fallback if the group signal is
/// rejected (e.g. the group already has no members). Never targets an
/// unrelated or reused pid.
fn kill_owned_process_group(pgid: libc::pid_t) {
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
        libc::kill(pgid, libc::SIGKILL);
    }
}

/// `None` when `XDG_RUNTIME_DIR` is absent: the backend must fail every
/// pick closed rather than guess an unsafe location (e.g. `/run/user/0`
/// for a non-root owner) or panic the daemon.
fn production_scratch_base() -> Option<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(runtime_dir).join("wayfinder-portal-picker"))
}

/// The two `parent_window` handle shapes the xdg-desktop-portal wire
/// protocol defines: `wayland:<xdg-foreign handle>` and `x11:<xid>`.
/// Empty (the app passed no parent) and any other shape are refused, so
/// nothing unvalidated from a D-Bus caller ever reaches the child's
/// argv.
fn is_forwardable_parent_handle(handle: &str) -> bool {
    let body = match handle
        .strip_prefix("wayland:")
        .or_else(|| handle.strip_prefix("x11:"))
    {
        Some(body) => body,
        None => return false,
    };
    !body.is_empty()
        // Never a value that reads as a flag if the child's argument
        // parser is ever changed to scan loosely.
        && !body.starts_with('-')
        && body.len() <= 128
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

impl Chooser for ProcessChooser {
    fn pick_single(&self, app_id: &str, parent_window: &str, title: &str) -> ChooserOutcome {
        self.pick(app_id, parent_window, title, &WireRequest::default())
            .outcome
    }

    fn pick(
        &self,
        _app_id: &str,
        parent_window: &str,
        _title: &str,
        request: &WireRequest,
    ) -> PickOutcome {
        let base = match &self.scratch_base {
            Some(base) => base,
            None => return PickOutcome::from(ChooserOutcome::Failed),
        };
        let root = match RequestRoot::create_under(base) {
            Ok(root) => root,
            Err(_) => return PickOutcome::from(ChooserOutcome::Failed),
        };
        let result_path = root.result_path();
        let request_path = root.request_path();
        let meta_path = root.result_meta_path();

        if write_request_file(&request_path, request).is_err() {
            return PickOutcome::from(ChooserOutcome::Failed);
        }

        let child = {
            let mut command = Command::new(&self.executable);
            command
                .arg("--pick")
                .arg("--out")
                .arg(&result_path)
                .arg("--request")
                .arg(&request_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if request.multiple {
                command.arg("--multiple");
            }
            // The calling app's exported toplevel handle, straight from
            // the FileChooser request. Without it the picker is a
            // parentless toplevel and the compositor has nothing to
            // stack it against, so it opens behind the app that asked
            // for it. Only a handle the wire protocol actually defines
            // is forwarded; anything else is dropped rather than passed
            // to the child as an opaque string.
            if is_forwardable_parent_handle(parent_window) {
                command.arg("--parent-window").arg(parent_window);
            }
            // Place the child in its own new process group (pgid == its
            // own pid), so a timeout kill can target that exact group --
            // covering any descendant it spawns -- without ever touching
            // an unrelated, pre-existing process group.
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command.spawn()
        };

        let child = match child {
            Ok(child) => child,
            Err(_) => {
                if self.should_treat_spawn_error_as_success() {
                    return PickOutcome::from(ChooserOutcome::Picked(vec![PathBuf::from(
                        "test-override-spawn-error-sentinel",
                    )]));
                }
                return PickOutcome::from(ChooserOutcome::Failed);
            }
        };

        let status = self.wait_with_deadline(child);
        let exit_code = status.and_then(|s| s.code());

        let outcome = if self.should_accept_any_exit_code() {
            // Same evidence, same exit code, but the classification guard
            // removed: any exit code at all now takes the success-shape
            // path, so an exit code that normally fails closed (e.g. `7`)
            // instead goes on to read the result as if it were `0`.
            match read_result_strict(&result_path) {
                Some(ParsedResult::Paths(paths)) => ChooserOutcome::Picked(paths),
                _ => ChooserOutcome::Failed,
            }
        } else {
            match exit_code {
                Some(0) => match read_result_strict(&result_path) {
                    Some(ParsedResult::Paths(paths)) => ChooserOutcome::Picked(paths),
                    _ if self.should_ignore_result_shape() => {
                        // Same bytes, same exit code, but the shape guard
                        // removed: fall back to the raw file contents as
                        // a single path instead of refusing malformed
                        // input.
                        fs::read_to_string(&result_path)
                            .ok()
                            .filter(|s| !s.is_empty())
                            .map(|s| ChooserOutcome::Picked(vec![PathBuf::from(s)]))
                            .unwrap_or(ChooserOutcome::Failed)
                    }
                    _ => ChooserOutcome::Failed,
                },
                Some(1) => match read_result_strict(&result_path) {
                    Some(ParsedResult::EmptyCancel) => ChooserOutcome::Cancelled,
                    _ => ChooserOutcome::Failed,
                },
                _ => ChooserOutcome::Failed,
            }
        };

        let meta = if matches!(outcome, ChooserOutcome::Picked(_)) {
            read_result_meta(&meta_path)
        } else {
            WireResultMeta::default()
        };

        if self.should_skip_cleanup() {
            // Same completed request, but the cleanup guard removed: do
            // not let `RequestRoot::drop` run, so the exact root that
            // normally vanishes with the outcome instead survives it.
            std::mem::forget(root);
        }

        PickOutcome { outcome, meta }
    }
}

fn refuse() -> (u32, HashMap<String, OwnedValue>) {
    (RESPONSE_OTHER, HashMap::new())
}

fn option_is_true(options: &HashMap<String, OwnedValue>, key: &str) -> bool {
    options
        .get(key)
        .and_then(|v| bool::try_from(v.clone()).ok())
        .unwrap_or(false)
}

/// Builds the final `OpenFile` results map for one or more picked URIs,
/// adding `results.current_filter`/`results.choices` only when the request
/// actually supplied filters/choices (an empty-filters request must never
/// grow a `current_filter` key, and likewise for choices).
fn build_success_results(
    uris: Vec<String>,
    meta: &WireResultMeta,
) -> (u32, HashMap<String, OwnedValue>) {
    let mut results = HashMap::new();
    results.insert(
        "uris".to_string(),
        OwnedValue::try_from(Value::from(uris)).expect("Vec<String> always encodes"),
    );
    if let Some(filter) = &meta.current_filter {
        let encoded = (filter.name.clone(), filter.rules.clone());
        results.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(encoded)).expect("filter struct always encodes"),
        );
    }
    if !meta.choices.is_empty() {
        results.insert(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(meta.choices.clone()))
                .expect("choice pairs always encode"),
        );
    }
    (RESPONSE_SUCCESS, results)
}

/// Decodes the real `a(sa(us))` `filters` wire value. `Ok(vec![])` when
/// the key is absent; `Err(())` only when the key is present but
/// structurally malformed -- never a panic.
fn decode_filters(options: &HashMap<String, OwnedValue>) -> Result<Vec<WireFilter>, ()> {
    let Some(value) = options.get("filters") else {
        return Ok(Vec::new());
    };
    let raw: Vec<(String, Vec<(u32, String)>)> =
        Vec::try_from(value.clone()).map_err(|_| ())?;
    Ok(raw
        .into_iter()
        .map(|(name, rules)| WireFilter { name, rules })
        .collect())
}

/// Decodes the real `(sa(us))` `current_filter` wire value. When `filters`
/// is nonempty, resolves it against that list: an exact match selects that
/// filter, anything else (absent, or simply not found) falls back to the
/// first supplied filter. When `filters` is empty, `current_filter` is
/// decoded and returned on its own -- the caller supplied exactly one
/// filter via `current_filter` alone, which becomes the effective active
/// filter without a fake entry ever appearing in `filters` -- or `None`
/// when the key itself is absent. Returns `Err(())` only when the key is
/// present but structurally malformed.
fn decode_current_filter(
    options: &HashMap<String, OwnedValue>,
    filters: &[WireFilter],
) -> Result<Option<WireFilter>, ()> {
    let decoded: Option<WireFilter> = match options.get("current_filter") {
        None => None,
        Some(value) => {
            let (name, rules): (String, Vec<(u32, String)>) =
                <(String, Vec<(u32, String)>)>::try_from(value.clone()).map_err(|_| ())?;
            Some(WireFilter { name, rules })
        }
    };
    if filters.is_empty() {
        return Ok(decoded);
    }
    let resolved = decoded
        .and_then(|cf| filters.iter().find(|f| f.name == cf.name && f.rules == cf.rules).cloned());
    Ok(Some(resolved.unwrap_or_else(|| filters[0].clone())))
}

/// Decodes the real `a(ssa(ss)s)` `choices` wire value, enforcing the
/// installed shape rather than silently inventing values: a choice's `id`
/// and `label` must be nonempty, and so must every option's id and label.
/// `initial` of `""` means the portal chooses -- the first option for an
/// enumerated choice, `"false"` for a boolean one. A nonempty enumerated
/// `initial` must name one of that choice's own option ids; a boolean
/// `initial` must be `""`, `"true"` or `"false"`. Any violation is
/// `Err(())`, refusing the whole request before the chooser is ever
/// called -- never a default standing in for a caller's bad input.
/// `Ok(vec![])` when the key is absent; result pairs stay in request
/// order.
fn decode_choices(options: &HashMap<String, OwnedValue>) -> Result<Vec<WireChoice>, ()> {
    let Some(value) = options.get("choices") else {
        return Ok(Vec::new());
    };
    let raw: Vec<(String, String, Vec<(String, String)>, String)> =
        Vec::try_from(value.clone()).map_err(|_| ())?;
    raw.into_iter()
        .map(|(id, label, options, initial)| {
            if id.is_empty() || label.is_empty() {
                return Err(());
            }
            if options.iter().any(|(oid, olabel)| oid.is_empty() || olabel.is_empty()) {
                return Err(());
            }
            let resolved_initial = if options.is_empty() {
                match initial.as_str() {
                    "" | "false" => "false".to_string(),
                    "true" => "true".to_string(),
                    _ => return Err(()),
                }
            } else if initial.is_empty() {
                options[0].0.clone()
            } else if options.iter().any(|(oid, _)| *oid == initial) {
                initial
            } else {
                return Err(());
            };
            Ok(WireChoice { id, label, options, initial: resolved_initial })
        })
        .collect()
}

/// Decodes a `current_folder` / `current_file` option. The wire type is
/// `ay`: a byte string, NUL-terminated by the C portal, so trailing NULs
/// are stripped. A plain `s` is accepted too, because some callers send
/// one. Everything else, and anything that is not valid UTF-8, not
/// absolute, or carries an interior NUL, is `Err(())` -- a suggestion
/// that cannot be trusted refuses the request rather than being quietly
/// dropped, since silently ignoring it would save the file somewhere the
/// app did not ask for.
fn decode_path_option(options: &HashMap<String, OwnedValue>, key: &str) -> Result<Option<String>, ()> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    let bytes: Vec<u8> = match Vec::<u8>::try_from(value.clone()) {
        Ok(bytes) => bytes,
        Err(_) => match String::try_from(value.clone()) {
            Ok(text) => text.into_bytes(),
            Err(_) => return Err(()),
        },
    };
    let trimmed: &[u8] = match bytes.iter().position(|b| *b == 0) {
        Some(first_nul) => {
            // Only trailing NULs are tolerated: an interior NUL means the
            // value is not one path.
            if bytes[first_nul..].iter().any(|b| *b != 0) {
                return Err(());
            }
            &bytes[..first_nul]
        }
        None => &bytes,
    };
    if trimmed.is_empty() {
        return Err(());
    }
    let text = std::str::from_utf8(trimmed).map_err(|_| ())?;
    if !Path::new(text).is_absolute() {
        return Err(());
    }
    Ok(Some(text.to_string()))
}

/// Decodes the `current_name` option: the file name the app suggests.
/// It must be one plain name -- no separator, no NUL, never "." or ".."
/// -- so a suggestion can never redirect the save out of the folder the
/// user is looking at. Absent is `Ok("")`.
fn decode_current_name(options: &HashMap<String, OwnedValue>) -> Result<String, ()> {
    let Some(value) = options.get("current_name") else {
        return Ok(String::new());
    };
    let name = String::try_from(value.clone()).map_err(|_| ())?;
    if name.is_empty() {
        return Ok(String::new());
    }
    if name.contains('/') || name.contains('\0') || name == "." || name == ".." {
        return Err(());
    }
    Ok(name)
}

/// Which FileChooser method a `decide` call is answering. Save differs
/// from Open in exactly two places, and both are decided here rather
/// than by sniffing the options: which options are decoded, and what a
/// returned path has to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestShape {
    Open,
    Save,
}

/// The result guard for a Save: the named file does NOT have to exist --
/// that is the whole point -- but it must be a path that can actually be
/// written as a file. So it must be absolute, must not already be a
/// directory, and its parent must be an existing directory. Without the
/// parent check a save into a folder that was deleted while the picker
/// was open would be reported to the app as success.
fn save_target_is_writable(candidate: &Path) -> bool {
    if !candidate.is_absolute() {
        return false;
    }
    if candidate.is_dir() {
        return false;
    }
    match candidate.file_name().and_then(|name| name.to_str()) {
        Some(name) if !name.is_empty() => {}
        _ => return false,
    }
    candidate.parent().is_some_and(|parent| parent.is_dir())
}

/// Every remaining stage-one option/result refusal is named here so a
/// mutation proof can force exactly one of them off at a time and show
/// the request then wrongly succeeds -- proving that guard, and only
/// that guard, is what was blocking it. Production's one real call site
/// always passes `GuardOverride::None`; nothing reachable from a D-Bus
/// caller, an option value or an environment variable can set anything
/// else, so this is not a backdoor, only a named decision point for
/// tests calling the Rust function directly. `multiple`, `filters`,
/// `current_filter` and `choices` are decoded and honored as of D-222 and
/// no longer have guards here; `directory` remains refused outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuardOverride {
    None,
    SkipDirectory,
    SkipCardinality,
    SkipMissingPath,
    SkipInvalidOutput,
    /// Waives only the Save result guard (`save_target_is_writable`), so a
    /// mutation proof can show that an unwritable Save target is refused
    /// by that guard and nothing else.
    SkipSaveTarget,
}

/// `pick` is called at most once, and only after every option guard has
/// passed and the request has been fully decoded -- so a refused or
/// malformed request never invokes the chooser seam.
pub fn decide(
    options: &HashMap<String, OwnedValue>,
    pick: impl FnOnce(&WireRequest) -> PickOutcome,
    override_guard: GuardOverride,
) -> (u32, HashMap<String, OwnedValue>) {
    decide_with_shape(options, pick, override_guard, RequestShape::Open)
}

/// `decide` for either method. The Open path is byte-for-byte what it
/// always was; Save decodes three more options and swaps the
/// "the picked file must already exist" guard for "the named file must be
/// writable", because a Save names a file that usually does not exist yet.
pub fn decide_with_shape(
    options: &HashMap<String, OwnedValue>,
    pick: impl FnOnce(&WireRequest) -> PickOutcome,
    override_guard: GuardOverride,
    shape: RequestShape,
) -> (u32, HashMap<String, OwnedValue>) {
    let save = shape == RequestShape::Save;
    if override_guard != GuardOverride::SkipDirectory && option_is_true(options, "directory") {
        return refuse();
    }

    // A Save names exactly one file; `multiple` is meaningless there and is
    // never honored, so a caller cannot ask for several save targets.
    let multiple = !save && option_is_true(options, "multiple");
    let (current_name, current_folder, current_file) = if save {
        let Ok(name) = decode_current_name(options) else {
            return refuse();
        };
        let Ok(folder) = decode_path_option(options, "current_folder") else {
            return refuse();
        };
        let Ok(file) = decode_path_option(options, "current_file") else {
            return refuse();
        };
        (name, folder, file)
    } else {
        (String::new(), None, None)
    };
    let Ok(filters) = decode_filters(options) else {
        return refuse();
    };
    let Ok(current_filter) = decode_current_filter(options, &filters) else {
        return refuse();
    };
    let Ok(choices) = decode_choices(options) else {
        return refuse();
    };

    let request = WireRequest {
        multiple,
        filters,
        current_filter,
        choices,
        save,
        current_name,
        current_folder,
        current_file,
    };
    let pick_outcome = pick(&request);
    // The chooser's own reported meta wins when present (a real picker may
    // let the user switch the active filter or a choice away from its
    // resolved default); otherwise this falls back to the request's own
    // already-resolved default, so even a chooser that only implements the
    // older `pick_single` seam still returns a correct `current_filter`/
    // `choices` for a request that supplied them.
    let meta = WireResultMeta {
        current_filter: pick_outcome.meta.current_filter.clone().or_else(|| request.current_filter.clone()),
        choices: if pick_outcome.meta.choices.is_empty() {
            request.choices.iter().map(|c| (c.id.clone(), c.initial.clone())).collect()
        } else {
            pick_outcome.meta.choices.clone()
        },
    };

    match pick_outcome.outcome {
        ChooserOutcome::Cancelled => (RESPONSE_CANCELLED, HashMap::new()),
        ChooserOutcome::Failed => refuse(),
        ChooserOutcome::Picked(paths) if multiple => {
            if paths.is_empty() {
                return refuse();
            }
            let mut uris = Vec::with_capacity(paths.len());
            for candidate in &paths {
                if override_guard != GuardOverride::SkipMissingPath
                    && candidate.is_absolute()
                    && !candidate.is_file()
                {
                    return refuse();
                }
                match normalize_to_single_file_uri(candidate) {
                    Some(uri) => uris.push(uri),
                    None if override_guard == GuardOverride::SkipInvalidOutput => {
                        uris.push("file:///forced-bypass-proof".to_string());
                    }
                    None => return refuse(),
                }
            }
            build_success_results(uris, &meta)
        }
        ChooserOutcome::Picked(paths) => {
            if override_guard != GuardOverride::SkipCardinality && paths.len() != 1 {
                return refuse();
            }
            let candidate = paths.first().cloned().unwrap_or_default();

            // The missing-path guard is its own decision point, checked
            // before URI normalization, and it applies only to an
            // otherwise-local absolute candidate: a relative candidate is
            // always refused by the invalid-output normalization guard
            // below instead, never here. That keeps the two guards
            // disjoint -- `SkipMissingPath` cannot also waive invalid
            // output, and `SkipInvalidOutput` cannot also waive a missing
            // file, so overriding one can never incidentally cover the
            // other's case.
            if save {
                // A Save target is judged by `save_target_is_writable`, not by
                // already existing. The two guards never both run, so neither
                // can stand in for the other.
                if override_guard != GuardOverride::SkipSaveTarget
                    && !save_target_is_writable(&candidate)
                {
                    return refuse();
                }
            } else if override_guard != GuardOverride::SkipMissingPath
                && candidate.is_absolute()
                && !candidate.is_file()
            {
                return refuse();
            }

            if override_guard == GuardOverride::SkipInvalidOutput {
                let uri = normalize_to_single_file_uri(&candidate)
                    .unwrap_or_else(|| "file:///forced-bypass-proof".to_string());
                return build_success_results(vec![uri], &meta);
            }
            match normalize_to_single_file_uri(&candidate) {
                Some(uri) => build_success_results(vec![uri], &meta),
                None => refuse(),
            }
        }
    }
}

/// Normalizes one picked local path to exactly one `file://` URI using
/// GIO URI construction (never string-prefixing), so spaces and
/// non-ASCII bytes are percent-encoded by the platform API rather than
/// this code. `None` for anything non-local, relative or otherwise
/// unnormalizable. Existence is checked by its own guard in `decide`,
/// not here.
fn normalize_to_single_file_uri(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let file = gio::File::for_path(path);
    let uri = file.uri();
    if !uri.starts_with("file://") {
        return None;
    }
    Some(uri.to_string())
}

pub struct PortalService {
    chooser: Arc<dyn Chooser>,
}

impl PortalService {
    pub fn new(chooser: Arc<dyn Chooser>) -> Self {
        Self { chooser }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl PortalService {
    #[zbus(name = "OpenFile")]
    fn open_file(
        &self,
        _handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let chooser = &self.chooser;
        decide(
            &options,
            |request| chooser.pick(&app_id, &parent_window, &title, request),
            GuardOverride::None,
        )
    }

    /// `SaveFile`: the app names a file to write. Same request plumbing and
    /// same chooser process as `OpenFile`; the request carries `save: true`
    /// and the suggested name/folder/file, and the result is guarded as a
    /// writable target rather than an existing one.
    #[zbus(name = "SaveFile")]
    fn save_file(
        &self,
        _handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let chooser = &self.chooser;
        decide_with_shape(
            &options,
            |request| chooser.pick(&app_id, &parent_window, &title, request),
            GuardOverride::None,
            RequestShape::Save,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use zbus::blocking::{connection::Builder, Connection};

    /// Serializes every test in this module that mutates `TMPDIR` or
    /// `XDG_RUNTIME_DIR` (process-wide state), so `cargo test`'s default
    /// parallel threads never race each other's environment.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A fresh private session bus under a uniquely named root below
    /// `~/Projects/Scratch/`. `TMPDIR` and `XDG_RUNTIME_DIR` point inside
    /// that root for the life of the guard; both, and the bus socket, are
    /// removed on drop -- success, failure or panic (the `Drop` impl is
    /// unwind-safe).
    struct PrivateBus {
        root: PathBuf,
        address: String,
        daemon: Child,
        old_tmpdir: Option<String>,
        old_xdg_runtime_dir: Option<String>,
        old_dbus_session_bus_address: Option<String>,
        /// Set once `finish_private_bus` has explicitly removed `root`, so
        /// `Drop` never attempts a second removal of a path that may since
        /// have been reused.
        root_removed: Cell<bool>,
    }

    impl PrivateBus {
        fn start() -> Self {
            let home = std::env::var("HOME").expect("HOME must be set");
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let root = PathBuf::from(home)
                .join("Projects/Scratch")
                .join(format!("d201-portal-test-{}-{}", std::process::id(), stamp));
            let run_dir = root.join("run");
            fs::create_dir_all(&run_dir).expect("create scratch root");
            fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700))
                .expect("restrict scratch run dir");
            let socket_path = root.join("bus.sock");

            let mut daemon = Command::new("dbus-daemon")
                .arg("--session")
                .arg(format!("--address=unix:path={}", socket_path.display()))
                .arg("--nofork")
                .arg("--print-address")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn a private dbus-daemon");
            let mut reader = BufReader::new(daemon.stdout.take().expect("daemon stdout piped"));
            let mut address = String::new();
            reader
                .read_line(&mut address)
                .expect("read private bus address");
            let address = address.trim().to_string();
            assert!(
                !address.is_empty(),
                "private dbus-daemon printed no address"
            );

            let old_tmpdir = std::env::var("TMPDIR").ok();
            let old_xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
            let old_dbus_session_bus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
            std::env::set_var("TMPDIR", &root);
            std::env::set_var("XDG_RUNTIME_DIR", &run_dir);
            // Any child the test spawns (the real native picker driver
            // included) must never be able to fall back to the real
            // session bus; point this at the private one explicitly
            // rather than relying on every spawn site to pass it.
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);

            Self {
                root,
                address,
                daemon,
                old_tmpdir,
                old_xdg_runtime_dir,
                old_dbus_session_bus_address,
                root_removed: Cell::new(false),
            }
        }

        fn connect_server(&self, chooser: Arc<dyn Chooser>) -> Connection {
            let service = PortalService::new(chooser);
            Builder::address(self.address.as_str())
                .expect("valid private bus address")
                .name(BUS_NAME)
                .expect("own the backend bus name on the private bus")
                .serve_at(OBJECT_PATH, service)
                .expect("export the service object")
                .build()
                .expect("build the server connection")
        }

        fn connect_client(&self) -> Connection {
            Builder::address(self.address.as_str())
                .expect("valid private bus address")
                .build()
                .expect("build the client connection")
        }
    }

    impl PrivateBus {
        fn root(&self) -> &Path {
            &self.root
        }

        /// Writes a real, round-owned fixture file inside this bus's own
        /// scratch root and returns its path. Never under `/tmp`.
        fn create_fixture_file(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, b"d202 fixture").expect("write round-owned fixture file");
            path
        }
    }

    /// Kills the daemon, restores `TMPDIR`/`XDG_RUNTIME_DIR`, and -- unless
    /// `finish_private_bus` already removed it explicitly -- makes a
    /// bounded best-effort removal of `root` itself, ignoring the result.
    /// That fallback is what recovers the root when a test panics before
    /// reaching `finish_private_bus`; it never panics itself, so a second
    /// panic during unwind can never turn into a process abort.
    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
            match &self.old_tmpdir {
                Some(v) => std::env::set_var("TMPDIR", v),
                None => std::env::remove_var("TMPDIR"),
            }
            match &self.old_xdg_runtime_dir {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
            match &self.old_dbus_session_bus_address {
                Some(v) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v),
                None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
            }
            if !self.root_removed.get() {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    /// Closes the connections and the private bus, then explicitly removes
    /// its round-owned scratch root. Failure to remove it fails the test;
    /// `Drop`'s own fallback removal is skipped afterward since
    /// `root_removed` is set first.
    fn finish_private_bus(bus: PrivateBus, server: Connection, client: Connection) {
        let root = bus.root().to_path_buf();
        drop(client);
        drop(server);
        fs::remove_dir_all(&root).expect("remove the round-owned scratch root");
        bus.root_removed.set(true);
        drop(bus);
    }

    /// A small round-owned scratch fixture for pure `decide()` tests that
    /// need one real file to exist, created under `~/Projects/Scratch/`
    /// (never `/tmp`). Callers normally call `remove()` explicitly so a
    /// cleanup failure fails the test rather than being ignored; `Drop`
    /// makes a bounded best-effort removal otherwise, which is what
    /// recovers the fixture dir if the test panics first.
    struct ScratchFile {
        dir: PathBuf,
        path: PathBuf,
        removed: Cell<bool>,
    }

    impl ScratchFile {
        fn create() -> Self {
            let home = std::env::var("HOME").expect("HOME must be set");
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let dir = PathBuf::from(home)
                .join("Projects/Scratch")
                .join(format!("d202-fixture-{}-{}", std::process::id(), stamp));
            fs::create_dir_all(&dir).expect("create scratch fixture dir");
            let path = dir.join("picked.txt");
            fs::write(&path, b"d202 fixture").expect("write scratch fixture file");
            Self {
                dir,
                path,
                removed: Cell::new(false),
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        /// Explicitly, observably removes the fixture dir. Failure fails
        /// the test; `Drop`'s fallback removal is skipped afterward since
        /// `removed` is set first.
        fn remove(self) {
            fs::remove_dir_all(&self.dir).expect("remove the round-owned scratch fixture dir");
            self.removed.set(true);
        }
    }

    /// Unwind-safe fallback: a bounded best-effort removal, ignored if it
    /// fails, and skipped entirely when `remove()` already removed it. Never
    /// panics, so it never turns an in-flight panic into a double panic.
    impl Drop for ScratchFile {
        fn drop(&mut self) {
            if !self.removed.get() {
                let _ = fs::remove_dir_all(&self.dir);
            }
        }
    }

    /// A tiny, test-only stand-in for `wayfinder-native`'s `--pick --out`
    /// contract: a disposable shell script under `~/Projects/Scratch/`
    /// (never `/tmp`), removed on drop. `ProcessChooser` spawns it exactly
    /// as it would spawn the real binary -- a real process boundary, not
    /// a faked decision.
    struct FakeNative {
        dir: PathBuf,
        script: PathBuf,
    }

    /// A process-wide counter, so two threads whose `SystemTime::now()`
    /// calls land on the same clock tick still never collide on the same
    /// scratch path -- the nanosecond stamp alone is not collision-safe
    /// under parallel test execution.
    fn next_unique_id() -> u64 {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    impl FakeNative {
        fn scripted(body: &str) -> Self {
            let home = std::env::var("HOME").expect("HOME must be set");
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let dir = PathBuf::from(home).join("Projects/Scratch").join(format!(
                "d205-fake-native-{}-{}-{}",
                std::process::id(),
                stamp,
                next_unique_id()
            ));
            fs::create_dir_all(&dir).expect("create fake native dir");
            let script = dir.join("fake-native.sh");
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write fake native script");
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
                .expect("chmod fake native script executable");
            Self { dir, script }
        }

        fn path(&self) -> &Path {
            &self.script
        }
    }

    impl Drop for FakeNative {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A fresh, uniquely named scratch base under `~/Projects/Scratch/`
    /// for one test's `ProcessChooser::for_test` root -- never `/tmp`.
    fn scratch_base(label: &str) -> PathBuf {
        let home = std::env::var("HOME").expect("HOME must be set");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        PathBuf::from(home).join("Projects/Scratch").join(format!(
            "d205-{}-{}-{}-{}",
            label,
            std::process::id(),
            stamp,
            next_unique_id()
        ))
    }

    fn count_fake_native_processes(script: &Path) -> usize {
        let output = Command::new("pgrep")
            .arg("-f")
            .arg(script.to_string_lossy().to_string())
            .output()
            .expect("run pgrep");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    }

    /// Removes the exact base dir a test asked `scratch_base()` for.
    /// Checked: a cleanup failure here fails the test rather than being
    /// swallowed, so a leaked root is always test-visible.
    fn finish_base(base: PathBuf) {
        fs::remove_dir_all(&base).expect("remove the exact test-owned scratch base root");
    }

    #[test]
    fn process_chooser_success_passes_through_one_picked_path() {
        let fixture = ScratchFile::create();
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        ));
        let base = scratch_base("success");
        let chooser =
            ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        match outcome {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!("expected Picked, got {other:?}"),
        }
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be removed after a completed pick"
        );
        fixture.remove();
        finish_base(base);
    }

    #[test]
    fn production_scratch_base_fails_closed_without_panic_when_runtime_dir_absent() {
        let _guard = env_lock();
        let saved = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::remove_var("XDG_RUNTIME_DIR");

        let found = production_scratch_base();

        if let Some(saved) = saved {
            std::env::set_var("XDG_RUNTIME_DIR", saved);
        }

        assert!(
            found.is_none(),
            "an absent XDG_RUNTIME_DIR must resolve to no root, never a guessed location"
        );
    }

    #[test]
    fn process_chooser_with_no_scratch_base_fails_closed_without_spawning() {
        let marker_dir = scratch_base("no-base-marker");
        fs::create_dir_all(&marker_dir).expect("create marker dir");
        let marker = marker_dir.join("launched");
        let native = FakeNative::scripted(&format!(
            ": > \"$3\"\nprintf 'x' >> '{}'\nexit 0\n",
            marker.display()
        ));
        let chooser = ProcessChooser {
            executable: native.path().to_path_buf(),
            deadline: Duration::from_secs(5),
            scratch_base: None,
            test_override: TestOverride::default(),
        };
        let outcome = chooser.pick_single("app", "", "title");
        assert!(
            matches!(outcome, ChooserOutcome::Failed),
            "no owner-only root means fail closed, not an attempt"
        );
        assert!(!marker.exists(), "no root means the child must never be launched");
        fs::remove_dir_all(&marker_dir).expect("remove exact marker fixture");
    }

    #[test]
    fn process_chooser_scratch_base_is_owner_only_mode() {
        let fixture = ScratchFile::create();
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        ));
        let base = scratch_base("mode");
        let chooser =
            ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let _ = chooser.pick_single("app", "", "title");
        let mode = fs::metadata(&base).expect("base must exist after a pick").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "the scratch base itself must be owner-only, not just its req-* subdir");
        fixture.remove();
        finish_base(base);
    }

    /// A real confirmed cancel: exit `1` *and* an exactly empty result
    /// file, matching the native contract precisely.
    #[test]
    fn process_chooser_exit_one_with_empty_result_maps_to_cancelled() {
        let native = FakeNative::scripted(": > \"$3\"\nexit 1\n");
        let base = scratch_base("cancel");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Cancelled), "got {outcome:?}");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        finish_base(base);
    }

    /// Exit `1` with no result file written at all is not a confirmed
    /// cancel -- the native contract always writes the empty file -- so
    /// this must fail closed rather than being misread as Cancel.
    #[test]
    fn process_chooser_exit_one_without_result_file_fails_closed_not_cancelled() {
        let native = FakeNative::scripted("exit 1\n");
        let base = scratch_base("cancel-no-write");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    /// Exit `1` with a nonempty result file is malformed, not a cancel.
    #[test]
    fn process_chooser_exit_one_with_nonempty_result_fails_closed() {
        let native = FakeNative::scripted("printf 'should-not-be-here' > \"$3\"\nexit 1\n");
        let base = scratch_base("cancel-nonempty");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    #[test]
    fn process_chooser_child_killed_by_signal_fails_closed() {
        let native = FakeNative::scripted("kill -9 $$\n");
        let base = scratch_base("signal");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        finish_base(base);
    }

    /// The owned child's own descendant (the `sleep`, spawned by the
    /// shell) must not outlive a timeout kill as an orphan: both the
    /// shell and the `sleep` it spawned are in the same owned process
    /// group, so the group-wide `SIGKILL` must reap both.
    #[test]
    fn process_chooser_deadline_expiry_reaps_child_and_its_descendant() {
        // The sleeper's own pid is written one level above the request
        // root (into the persistent scratch base, via `$3`'s grandparent)
        // so the exact descendant identity survives `RequestRoot::drop`
        // and can be checked by pid -- not by a name/pattern match that
        // an unrelated system `sleep 5` could satisfy or fail.
        let native = FakeNative::scripted(
            "sleep 5 &\nchildpid=$!\necho $childpid > \"$(dirname \"$3\")/../sleeppid\"\nwait $childpid\nexit 0\n",
        );
        let base = scratch_base("deadline");
        let chooser =
            ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_millis(200), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        assert_eq!(
            count_fake_native_processes(native.path()),
            0,
            "the shell must be reaped after the deadline"
        );
        let sleep_pid: libc::pid_t = fs::read_to_string(base.join("sleeppid"))
            .expect("read the exact sleeper pid left by the fixture")
            .trim()
            .parse()
            .expect("sleeper pid must parse");
        // `SIGKILL` to the group reaches the orphaned `sleep` immediately,
        // but its eventual reaper (the init process it was reparented to)
        // collects the zombie asynchronously -- so poll with a short
        // bound rather than asserting on the very next instant. `kill -0`
        // on this one exact pid, not a name/pattern search, so a reused
        // pid or an unrelated system sleeper can neither fail nor satisfy
        // this check.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut still_alive = true;
        while Instant::now() < deadline {
            still_alive = unsafe { libc::kill(sleep_pid, 0) } == 0;
            if !still_alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !still_alive,
            "the shell's own sleep descendant (exact pid {sleep_pid}) must not survive as an orphan after the group kill"
        );
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1, "only the leftover sleeppid marker, the req-* root is gone");
        fs::remove_file(base.join("sleeppid")).expect("remove exact sleeppid marker");
        finish_base(base);
    }

    #[test]
    fn process_chooser_success_exit_without_result_file_fails_closed() {
        let native = FakeNative::scripted("exit 0\n");
        let base = scratch_base("no-result");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        finish_base(base);
    }

    #[test]
    fn process_chooser_malformed_result_missing_trailing_newline_fails_closed() {
        let native = FakeNative::scripted("printf '%s' '/some/path' > \"$3\"\nexit 0\n");
        let base = scratch_base("malformed");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    #[test]
    fn process_chooser_malformed_result_blank_record_fails_closed() {
        let native = FakeNative::scripted("printf '/a\\n\\n/b\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("blank-record");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    #[test]
    fn process_chooser_malformed_result_invalid_utf8_fails_closed() {
        let native = FakeNative::scripted("printf '\\377\\376\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("invalid-utf8");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    #[test]
    fn process_chooser_malformed_result_oversized_fails_closed() {
        let native = FakeNative::scripted(
            "yes /oversized-d205-path | head -c 70000 > \"$3\"\nexit 0\n",
        );
        let base = scratch_base("oversized");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        finish_base(base);
    }

    #[test]
    fn process_chooser_malformed_result_unreadable_fails_closed() {
        let native = FakeNative::scripted(
            "printf 'a\\n' > \"$3\"\nchmod 000 \"$3\"\nexit 0\n",
        );
        let base = scratch_base("unreadable");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(
            matches!(outcome, ChooserOutcome::Failed),
            "an exit-0 result file that cannot be opened must fail closed, got {outcome:?}"
        );
        finish_base(base);
    }

    /// The portal receives the caller's exported toplevel handle and
    /// must hand it to the picker child, or the picker is a parentless
    /// toplevel that the compositor leaves behind the app that asked.
    /// Records the child's real argv and asserts the flag and value are
    /// both there, adjacent.
    #[test]
    fn process_chooser_forwards_wayland_parent_handle_to_the_child() {
        let native = FakeNative::scripted(
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\nprintf '/tmp/x\\n' > \"$3\"\nexit 0\n",
        );
        let base = scratch_base("parent-handle-forwarded");
        let chooser =
            ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "wayland:handle-42", "title");
        assert!(matches!(outcome, ChooserOutcome::Picked(_)), "got {outcome:?}");
        let argv = read_recorded_argv(&native);
        let flag = argv
            .iter()
            .position(|a| a == "--parent-window")
            .expect("the child argv must carry --parent-window");
        assert_eq!(
            argv.get(flag + 1).map(String::as_str),
            Some("wayland:handle-42"),
            "the handle must follow its flag unchanged, argv was {argv:?}"
        );
        finish_base(base);
    }

    /// An app that passes no parent (the empty handle) must not make the
    /// child receive a flag with nothing behind it, which the child would
    /// then have to guess at.
    #[test]
    fn process_chooser_omits_parent_window_flag_when_there_is_no_parent() {
        let native = FakeNative::scripted(
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\nprintf '/tmp/x\\n' > \"$3\"\nexit 0\n",
        );
        let base = scratch_base("parent-handle-absent");
        let chooser =
            ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Picked(_)), "got {outcome:?}");
        let argv = read_recorded_argv(&native);
        assert!(
            !argv.iter().any(|a| a == "--parent-window"),
            "no parent means no flag at all, argv was {argv:?}"
        );
        finish_base(base);
    }

    /// Only the two shapes the wire protocol defines are forwarded;
    /// anything else a D-Bus caller invents is dropped rather than
    /// handed to the child as an opaque argv value.
    #[test]
    fn only_protocol_shaped_parent_handles_are_forwardable() {
        for good in ["wayland:h", "x11:0x1400003", "wayland:a-b_c.d/e"] {
            assert!(is_forwardable_parent_handle(good), "{good} must be forwardable");
        }
        for bad in [
            "",
            "wayland:",
            "x11:",
            "handle-42",
            "wayland:h;rm -rf /",
            "wayland:h ",
            "wayland:--out",
        ] {
            assert!(!is_forwardable_parent_handle(bad), "{bad:?} must be refused");
        }
        assert!(
            !is_forwardable_parent_handle(&format!("wayland:{}", "h".repeat(129))),
            "an over-long handle must be refused"
        );
    }

    /// Reads the argv the fake native recorded beside its own script.
    /// It is recorded there, not in the request root, because the
    /// chooser deletes that root as soon as the pick returns.
    fn read_recorded_argv(native: &FakeNative) -> Vec<String> {
        let path = native
            .path()
            .parent()
            .expect("the fake native script has a directory")
            .join("argv");
        fs::read_to_string(&path)
            .expect("the fake native must have recorded its argv")
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn process_chooser_passes_through_multiple_paths_unfiltered() {
        let native = FakeNative::scripted("printf 'a\\nb\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("multi");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        match outcome {
            ChooserOutcome::Picked(paths) => assert_eq!(paths.len(), 2),
            other => panic!("expected Picked, got {other:?}"),
        }
        finish_base(base);
    }

    #[test]
    fn decide_refuses_process_chooser_multiple_paths_via_cardinality_guard() {
        let native = FakeNative::scripted("printf 'a\\nb\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("multi-decide");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let (response, results) = decide(
            &HashMap::new(),
            |_| chooser.pick_single("app", "", "title").into(),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_base(base);
    }

    #[test]
    fn decide_refuses_process_chooser_relative_output_via_invalid_output_guard_not_missing_path() {
        let native = FakeNative::scripted("printf 'relative/path.txt\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("relative-decide");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let (response, results) = decide(
            &HashMap::new(),
            |_| chooser.pick_single("app", "", "title").into(),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_base(base);

        let native2 = FakeNative::scripted("printf 'relative/path.txt\\n' > \"$3\"\nexit 0\n");
        let base2 = scratch_base("relative-decide-2");
        let chooser2 = ProcessChooser::for_test(native2.path().to_path_buf(), Duration::from_secs(5), base2.clone());
        let (response2, _) = decide(
            &HashMap::new(),
            |_| chooser2.pick_single("app", "", "title").into(),
            GuardOverride::SkipMissingPath,
        );
        assert_eq!(
            response2, RESPONSE_OTHER,
            "a relative candidate must still be refused when only the missing-path guard is bypassed"
        );
        finish_base(base2);
    }

    #[test]
    fn decide_refuses_process_chooser_missing_absolute_path_via_missing_path_guard() {
        let native = FakeNative::scripted(
            "printf '/nonexistent-d205-path/missing.txt\\n' > \"$3\"\nexit 0\n",
        );
        let base = scratch_base("missing-decide");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let (response, results) = decide(
            &HashMap::new(),
            |_| chooser.pick_single("app", "", "title").into(),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_base(base);
    }

    #[test]
    fn decide_refuses_process_chooser_directory_output() {
        let dir_fixture = scratch_base("dir-fixture");
        fs::create_dir_all(&dir_fixture).expect("create directory fixture");
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            dir_fixture.display()
        ));
        let base = scratch_base("dir-decide");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let (response, results) = decide(
            &HashMap::new(),
            |_| chooser.pick_single("app", "", "title").into(),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_base(base);
        fs::remove_dir_all(&dir_fixture).expect("remove exact directory-output fixture");
    }

    #[test]
    fn process_chooser_spawns_child_exactly_once_per_pick() {
        let base = scratch_base("count");
        let counter_dir = scratch_base("count-marker");
        fs::create_dir_all(&counter_dir).expect("create counter dir");
        let marker = counter_dir.join("launches");
        let native = FakeNative::scripted(&format!(": > \"$3\"\nprintf 'x' >> '{}'\nexit 1\n", marker.display()));
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let _ = chooser.pick_single("app", "", "title");
        let contents = fs::read_to_string(&marker).unwrap_or_default();
        assert_eq!(contents.len(), 1, "pick_single must spawn the child exactly once");
        finish_base(base);
        fs::remove_dir_all(&counter_dir).expect("remove exact launch-counter fixture");
    }

    #[test]
    fn process_chooser_reaps_child_on_normal_exit() {
        let native = FakeNative::scripted(": > \"$3\"\nexit 1\n");
        let base = scratch_base("reap");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let _ = chooser.pick_single("app", "", "title");
        assert_eq!(count_fake_native_processes(native.path()), 0);
        finish_base(base);
    }

    #[test]
    fn process_chooser_spawn_error_fails_closed() {
        let base = scratch_base("spawn-error");
        let chooser = ProcessChooser::for_test(
            PathBuf::from("/nonexistent-d205-executable"),
            Duration::from_secs(5),
            base.clone(),
        );
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Failed), "got {outcome:?}");
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "root must still be cleaned up on spawn error"
        );
        finish_base(base);
    }

    #[test]
    fn process_chooser_cleanup_removes_only_its_own_request_root() {
        let base = scratch_base("decoy");
        fs::create_dir_all(&base).expect("create shared scratch base");
        let decoy = base.join("decoy-unrelated-file.txt");
        fs::write(&decoy, b"must survive").expect("write decoy file");

        let native = FakeNative::scripted(": > \"$3\"\nexit 1\n");
        let chooser = ProcessChooser::for_test(native.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Cancelled), "got {outcome:?}");

        assert!(
            decoy.exists(),
            "cleanup must never remove anything outside its own owned request root"
        );
        let remaining: Vec<_> = fs::read_dir(&base).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(
            remaining,
            vec![decoy.clone()],
            "only the request's own root is removed; the decoy and nothing else remains"
        );
        fs::remove_dir_all(&base).expect("remove exact decoy fixture base");
    }

    /// Deletion oracle: the spawn step is load-bearing. Pointing the
    /// *same* executable argument at a real, unexecutable, non-regular
    /// path proves the spawn-error branch itself (not just "a bad
    /// executable string") is what turns this red; a working executable
    /// passed the same way succeeds, isolating the one decision.
    #[test]
    fn comparison_spawn_error_branch_differs_from_working_executable() {
        let base = scratch_base("oracle-spawn");
        let unexecutable_dir = scratch_base("oracle-spawn-target");
        fs::create_dir_all(&unexecutable_dir).expect("create non-executable target");
        let chooser =
            ProcessChooser::for_test(unexecutable_dir.clone(), Duration::from_secs(5), base.clone());
        let outcome = chooser.pick_single("app", "", "title");
        assert!(
            matches!(outcome, ChooserOutcome::Failed),
            "spawning a directory as the executable must fail closed via the spawn-error branch: got {outcome:?}"
        );
        finish_base(base);
        fs::remove_dir_all(&unexecutable_dir).expect("remove exact oracle fixture");
    }

    /// Deletion oracle: the deadline check is load-bearing. The exact
    /// same slow fixture that fails closed under a short deadline must
    /// succeed when the deadline is generous -- isolating the deadline
    /// comparison itself, not merely "a slow process fails."
    #[test]
    fn comparison_deadline_branch_differs_from_generous_deadline() {
        let fixture = ScratchFile::create();
        let slow_body = format!(
            "sleep 0.3\nprintf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        );

        let native_short = FakeNative::scripted(&slow_body);
        let base_short = scratch_base("oracle-deadline-short");
        let chooser_short = ProcessChooser::for_test(
            native_short.path().to_path_buf(),
            Duration::from_millis(50),
            base_short.clone(),
        );
        let outcome_short = chooser_short.pick_single("app", "", "title");
        assert!(
            matches!(outcome_short, ChooserOutcome::Failed),
            "a deadline shorter than the child's run time must fail closed: got {outcome_short:?}"
        );
        finish_base(base_short);

        let native_long = FakeNative::scripted(&slow_body);
        let base_long = scratch_base("oracle-deadline-long");
        let chooser_long = ProcessChooser::for_test(
            native_long.path().to_path_buf(),
            Duration::from_secs(5),
            base_long.clone(),
        );
        let outcome_long = chooser_long.pick_single("app", "", "title");
        match outcome_long {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!(
                "the same fixture under a generous deadline must succeed, isolating the deadline decision: got {other:?}"
            ),
        }
        finish_base(base_long);
        fixture.remove();
    }

    /// Deletion oracle: exit-code classification is load-bearing. The
    /// exact same result-file contents succeed at exit `0` and must fail
    /// closed at every other code, isolating the exit-code branch from
    /// the result-shape check.
    #[test]
    fn comparison_exit_classification_branch_differs_from_exit_zero() {
        let fixture = ScratchFile::create();
        let body = |code: i32| {
            format!(
                "printf '%s\\n' '{}' > \"$3\"\nexit {code}\n",
                fixture.path().display()
            )
        };

        let native_zero = FakeNative::scripted(&body(0));
        let base_zero = scratch_base("oracle-exit-zero");
        let chooser_zero =
            ProcessChooser::for_test(native_zero.path().to_path_buf(), Duration::from_secs(5), base_zero.clone());
        match chooser_zero.pick_single("app", "", "title") {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!("exit 0 with a well-formed single path must succeed: got {other:?}"),
        }
        finish_base(base_zero);

        let native_other = FakeNative::scripted(&body(7));
        let base_other = scratch_base("oracle-exit-other");
        let chooser_other = ProcessChooser::for_test(
            native_other.path().to_path_buf(),
            Duration::from_secs(5),
            base_other.clone(),
        );
        let outcome_other = chooser_other.pick_single("app", "", "title");
        assert!(
            matches!(outcome_other, ChooserOutcome::Failed),
            "the identical result file at an unrecognized exit code must fail closed, isolating the exit-code decision: got {outcome_other:?}"
        );
        finish_base(base_other);
        fixture.remove();
    }

    /// Deletion oracle: result-shape validation is load-bearing. The
    /// same exit code (`0`) succeeds for a well-formed single-path
    /// result and must fail closed for a malformed one, isolating the
    /// shape check from exit-code classification.
    #[test]
    fn comparison_result_shape_branch_differs_from_well_formed() {
        let fixture = ScratchFile::create();
        let native_ok = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        ));
        let base_ok = scratch_base("oracle-shape-ok");
        let chooser_ok =
            ProcessChooser::for_test(native_ok.path().to_path_buf(), Duration::from_secs(5), base_ok.clone());
        match chooser_ok.pick_single("app", "", "title") {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!("a well-formed result at exit 0 must succeed: got {other:?}"),
        }
        finish_base(base_ok);

        let native_malformed = FakeNative::scripted("printf 'no-trailing-newline' > \"$3\"\nexit 0\n");
        let base_malformed = scratch_base("oracle-shape-bad");
        let chooser_malformed = ProcessChooser::for_test(
            native_malformed.path().to_path_buf(),
            Duration::from_secs(5),
            base_malformed.clone(),
        );
        let outcome_malformed = chooser_malformed.pick_single("app", "", "title");
        assert!(
            matches!(outcome_malformed, ChooserOutcome::Failed),
            "the same exit code with a malformed result must fail closed, isolating the shape decision: got {outcome_malformed:?}"
        );
        finish_base(base_malformed);
        fixture.remove();
    }

    /// True deletion oracle: same evidence (the same spawn error, a real
    /// unexecutable directory as the executable), only the production
    /// spawn-error guard removed via `TestOverride`. The ordinary test
    /// above fails closed on this exact fixture; with only that guard
    /// bypassed, the outcome flips to `Picked`, proving the guard -- not
    /// the fixture -- was what made it fail.
    #[test]
    fn deletion_oracle_spawn_error_guard_bypass_turns_wrong() {
        let base = scratch_base("oracle-bypass-spawn");
        let unexecutable_dir = scratch_base("oracle-bypass-spawn-target");
        fs::create_dir_all(&unexecutable_dir).expect("create non-executable target");
        let chooser = ProcessChooser::for_test_with_override(
            unexecutable_dir.clone(),
            Duration::from_secs(5),
            base.clone(),
            TestOverride {
                treat_spawn_error_as_success: true,
                ..Default::default()
            },
        );
        let outcome = chooser.pick_single("app", "", "title");
        assert!(
            matches!(outcome, ChooserOutcome::Picked(_)),
            "with only the spawn-error guard bypassed, the identical unexecutable fixture must wrongly succeed: got {outcome:?}"
        );
        finish_base(base);
        fs::remove_dir_all(&unexecutable_dir).expect("remove exact oracle fixture");
    }

    /// True deletion oracle: the exact same slow fixture and the exact
    /// same short deadline as the ordinary comparison test above, only
    /// the deadline guard removed. The ordinary test fails closed under
    /// this deadline; with only the guard bypassed, it wrongly succeeds,
    /// isolating the deadline comparison itself from the fixture.
    #[test]
    fn deletion_oracle_deadline_guard_bypass_turns_wrong() {
        let fixture = ScratchFile::create();
        let slow_body = format!(
            "sleep 0.3\nprintf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        );
        let native = FakeNative::scripted(&slow_body);
        let base = scratch_base("oracle-bypass-deadline");
        let chooser = ProcessChooser::for_test_with_override(
            native.path().to_path_buf(),
            Duration::from_millis(50),
            base.clone(),
            TestOverride {
                ignore_deadline: true,
                ..Default::default()
            },
        );
        let outcome = chooser.pick_single("app", "", "title");
        match outcome {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!(
                "with only the deadline guard bypassed, the same too-slow fixture under the same short deadline must wrongly succeed: got {other:?}"
            ),
        }
        finish_base(base);
        fixture.remove();
    }

    /// True deletion oracle: the exact same exit code (`7`) and the exact
    /// same well-formed result bytes as the ordinary comparison test
    /// above, only the exit-classification guard removed. The ordinary
    /// test fails closed at this exit code; with only the guard
    /// bypassed, the identical evidence wrongly succeeds.
    #[test]
    fn deletion_oracle_exit_classification_guard_bypass_turns_wrong() {
        let fixture = ScratchFile::create();
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 7\n",
            fixture.path().display()
        ));
        let base = scratch_base("oracle-bypass-exit");
        let chooser = ProcessChooser::for_test_with_override(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
            TestOverride {
                accept_any_exit_code: true,
                ..Default::default()
            },
        );
        let outcome = chooser.pick_single("app", "", "title");
        match outcome {
            ChooserOutcome::Picked(paths) => assert_eq!(paths, vec![fixture.path().to_path_buf()]),
            other => panic!(
                "with only the exit-classification guard bypassed, the same exit-7 evidence must wrongly succeed: got {other:?}"
            ),
        }
        finish_base(base);
        fixture.remove();
    }

    /// True deletion oracle: the exact same malformed bytes (missing
    /// trailing newline) and the exact same exit code (`0`) as the
    /// ordinary comparison test above, only the result-shape guard
    /// removed. The ordinary test fails closed on this evidence; with
    /// only the guard bypassed, it wrongly succeeds.
    #[test]
    fn deletion_oracle_result_shape_guard_bypass_turns_wrong() {
        let native = FakeNative::scripted("printf 'no-trailing-newline' > \"$3\"\nexit 0\n");
        let base = scratch_base("oracle-bypass-shape");
        let chooser = ProcessChooser::for_test_with_override(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
            TestOverride {
                ignore_result_shape: true,
                ..Default::default()
            },
        );
        let outcome = chooser.pick_single("app", "", "title");
        assert!(
            matches!(outcome, ChooserOutcome::Picked(_)),
            "with only the result-shape guard bypassed, the same malformed bytes must wrongly succeed: got {outcome:?}"
        );
        finish_base(base);
    }

    /// True deletion oracle: the exact same completed, successful pick
    /// as the ordinary passing-path tests above, only the cleanup guard
    /// removed. Ordinarily the request root is gone immediately after;
    /// with only the guard bypassed, the identical completed request
    /// wrongly leaves its root behind.
    #[test]
    fn deletion_oracle_cleanup_guard_bypass_turns_wrong() {
        let fixture = ScratchFile::create();
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            fixture.path().display()
        ));
        let base = scratch_base("oracle-bypass-cleanup");
        let chooser = ProcessChooser::for_test_with_override(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
            TestOverride {
                skip_cleanup: true,
                ..Default::default()
            },
        );
        let outcome = chooser.pick_single("app", "", "title");
        assert!(matches!(outcome, ChooserOutcome::Picked(_)), "got {outcome:?}");
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            1,
            "with only the cleanup guard bypassed, the identical completed request must wrongly leave its root behind"
        );
        fixture.remove();
        fs::remove_dir_all(&base).expect("remove exact leaked root the bypass oracle proved would otherwise leak");
    }

    /// Deletion oracle: cleanup/reap is load-bearing and request-scoped.
    /// Two independent picks under the same base each leave their own
    /// root absent afterward -- proving cleanup fires per request, not
    /// as an accidental side effect of the base directory itself being
    /// torn down once at the end.
    #[test]
    fn deletion_oracle_cleanup_fires_independently_per_request() {
        let base = scratch_base("oracle-cleanup");
        let native_a = FakeNative::scripted(": > \"$3\"\nexit 1\n");
        let chooser_a = ProcessChooser::for_test(native_a.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let _ = chooser_a.pick_single("app", "", "title");
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the first request's root must already be gone before the second request starts"
        );

        let native_b = FakeNative::scripted(": > \"$3\"\nexit 1\n");
        let chooser_b = ProcessChooser::for_test(native_b.path().to_path_buf(), Duration::from_secs(5), base.clone());
        let _ = chooser_b.pick_single("app", "", "title");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        finish_base(base);
    }

    /// Forced-unwind oracle: a panic while a `RequestRoot` is alive must
    /// still remove the exact root via `Drop`, never leak it -- this is
    /// the real production cleanup contract, not merely a test-harness
    /// convention, so it is proven directly against `RequestRoot` itself.
    #[test]
    fn request_root_drop_recovers_exact_root_on_panic() {
        let base = scratch_base("forced-unwind-root");
        let dir_to_check = {
            let root = RequestRoot::create_under(&base).expect("create root");
            let dir = root.dir.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _keep_alive = &root;
                panic!("forced unwind while the request root is still alive");
            }));
            assert!(result.is_err(), "the inner closure must have actually panicked");
            dir
        };
        assert!(
            !dir_to_check.exists(),
            "RequestRoot::drop must remove the exact root even when unwinding from a panic"
        );
        finish_base(base);
    }

    /// Forced normal-path cleanup-failure oracle: proves `finish_base`'s
    /// checked removal is actually live -- a cleanup failure is test-
    /// visible (a panic), never silently swallowed.
    #[test]
    fn finish_base_panics_when_normal_cleanup_actually_fails() {
        let base = scratch_base("forced-cleanup-failure");
        fs::create_dir_all(&base).expect("create base");
        let blocker = base.join("blocked");
        fs::create_dir(&blocker).expect("create nested dir");
        // Strip write+exec from the nested dir's own permissions so an
        // entry inside it cannot be unlinked, forcing `remove_dir_all` to
        // fail partway through -- without touching `base`'s own mode, so
        // the eventual real cleanup below can still descend into it.
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o000)).expect("lock nested dir");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish_base(base.clone());
        }));
        assert!(
            result.is_err(),
            "finish_base must panic, not silently swallow, when the checked removal actually fails"
        );
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o700)).expect("unlock nested dir for real cleanup");
        fs::remove_dir_all(&base).expect("real cleanup after the forced-failure oracle");
    }

    /// D-203 oracle: panics inside the `PrivateBus` fixture's own scope
    /// (before any explicit `finish_private_bus` call), then proves the
    /// unwind-safe `Drop` fallback removed the exact captured root, reaped
    /// the private daemon, and restored `TMPDIR`/`XDG_RUNTIME_DIR`.
    #[test]
    fn private_bus_drop_recovers_root_on_panic() {
        let _guard = env_lock();
        let old_tmpdir = std::env::var("TMPDIR").ok();
        let old_xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
        let captured: Arc<Mutex<Option<(PathBuf, u32)>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let bus = PrivateBus::start();
            *captured_clone.lock().unwrap() = Some((bus.root().to_path_buf(), bus.daemon.id()));
            panic!("deliberate panic inside the PrivateBus fixture scope, before finish_private_bus");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let (root, daemon_pid) = captured
            .lock()
            .unwrap()
            .clone()
            .expect("root and daemon pid captured before the panic");
        assert!(
            !root.exists(),
            "PrivateBus::drop must remove its exact root on unwind"
        );
        assert!(
            !PathBuf::from(format!("/proc/{daemon_pid}")).exists(),
            "PrivateBus::drop must reap the private dbus-daemon on unwind"
        );
        assert_eq!(
            std::env::var("TMPDIR").ok(),
            old_tmpdir,
            "TMPDIR must be restored after the unwind"
        );
        assert_eq!(
            std::env::var("XDG_RUNTIME_DIR").ok(),
            old_xdg_runtime_dir,
            "XDG_RUNTIME_DIR must be restored after the unwind"
        );
    }

    /// D-203 oracle: panics inside the `ScratchFile` fixture's own scope
    /// (before any explicit `remove()` call), then proves the unwind-safe
    /// `Drop` fallback removed the exact captured fixture dir.
    #[test]
    fn scratch_file_drop_recovers_dir_on_panic() {
        let captured: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let fixture = ScratchFile::create();
            *captured_clone.lock().unwrap() = Some(fixture.dir.clone());
            panic!("deliberate panic inside the ScratchFile fixture scope, before remove()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let dir = captured
            .lock()
            .unwrap()
            .clone()
            .expect("fixture dir captured before the panic");
        assert!(
            !dir.exists(),
            "ScratchFile::drop must remove its exact fixture dir on unwind"
        );
    }

    /// D-205 oracle: panics inside an `OwnedScratchDir`'s own scope
    /// (before any explicit `finish()` call), then proves the unwind-safe
    /// `Drop` fallback removed the exact captured dir.
    #[test]
    fn owned_scratch_dir_drop_recovers_dir_on_panic() {
        let captured: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let owned = OwnedScratchDir::create(scratch_base("owned-dir-forced-unwind"));
            *captured_clone.lock().unwrap() = Some(owned.path().to_path_buf());
            panic!("deliberate panic inside the OwnedScratchDir scope, before finish()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let dir = captured
            .lock()
            .unwrap()
            .clone()
            .expect("owned dir path captured before the panic");
        assert!(
            !dir.exists(),
            "OwnedScratchDir::drop must remove its exact dir on unwind"
        );
    }

    /// Forced normal-path cleanup-failure oracle for `OwnedScratchDir`:
    /// proves `finish()`'s checked removal is actually live.
    #[test]
    fn owned_scratch_dir_finish_panics_when_normal_cleanup_actually_fails() {
        let dir = scratch_base("owned-dir-forced-cleanup-failure");
        let owned = OwnedScratchDir::create(dir.clone());
        let blocker = dir.join("blocked");
        fs::create_dir(&blocker).expect("create nested dir");
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o000)).expect("lock nested dir");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owned.finish();
        }));
        assert!(
            result.is_err(),
            "OwnedScratchDir::finish must panic, not silently swallow, when the checked removal actually fails"
        );
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o700)).expect("unlock nested dir for real cleanup");
        fs::remove_dir_all(&dir).expect("real cleanup after the forced-failure oracle");
    }

    /// D-205 oracle: panics inside a `PrivateXvfb`'s own scope (before
    /// any explicit `finish()` call), capturing only its pid and display
    /// number outside the panicking closure, then proves the unwind-safe
    /// `Drop` fallback reaped the exact process and removed its exact
    /// lock file and socket.
    #[test]
    fn private_xvfb_drop_recovers_process_and_lock_on_panic() {
        let captured: Arc<Mutex<Option<(u32, u32)>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let xvfb = PrivateXvfb::start();
            *captured_clone.lock().unwrap() = Some((xvfb.child.id(), xvfb.display_num));
            panic!("deliberate panic inside the PrivateXvfb fixture scope, before finish()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let (pid, display_num) = captured
            .lock()
            .unwrap()
            .clone()
            .expect("pid and display number captured before the panic");
        let deadline = Instant::now() + Duration::from_secs(5);
        while PathBuf::from(format!("/proc/{pid}")).exists() {
            assert!(Instant::now() < deadline, "PrivateXvfb::drop must reap the exact process on unwind");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !PrivateXvfb::lock_path_for(display_num).exists(),
            "PrivateXvfb::drop must leave no lock file behind on unwind"
        );
        assert!(
            !PrivateXvfb::socket_path_for(display_num).exists(),
            "PrivateXvfb::drop must leave no socket behind on unwind"
        );
    }

    /// Hands back exactly the scripted outcomes, in order, and counts calls
    /// so a test can prove a guard refused *before* the chooser was ever
    /// invoked, and that one call's outcome never leaks into the next.
    struct ScriptedChooser {
        outcomes: Mutex<VecDeque<ChooserOutcome>>,
        calls: Mutex<u32>,
    }

    impl ScriptedChooser {
        fn new(outcomes: Vec<ChooserOutcome>) -> Arc<Self> {
            Arc::new(Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: Mutex::new(0),
            })
        }

        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl Chooser for ScriptedChooser {
        fn pick_single(&self, _app_id: &str, _parent_window: &str, _title: &str) -> ChooserOutcome {
            *self.calls.lock().unwrap() += 1;
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ChooserOutcome::Failed)
        }
    }

    fn call_open_file(
        client: &Connection,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<(u32, HashMap<String, OwnedValue>)> {
        let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/t/1")
            .expect("literal object path");
        let body = (
            handle,
            "org.example.round211".to_string(),
            String::new(),
            "Open".to_string(),
            options,
        );
        let message = client.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE_NAME),
            "OpenFile",
            &body,
        )?;
        message
            .body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>()
            .map_err(Into::into)
    }

    fn call_save_file(
        client: &Connection,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<(u32, HashMap<String, OwnedValue>)> {
        let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/t/2")
            .expect("literal object path");
        let body = (
            handle,
            "org.example.save".to_string(),
            String::new(),
            "Save".to_string(),
            options,
        );
        let message = client.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE_NAME),
            "SaveFile",
            &body,
        )?;
        message
            .body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>()
            .map_err(Into::into)
    }

    fn uris_of(results: &HashMap<String, OwnedValue>) -> Vec<String> {
        results
            .get("uris")
            .cloned()
            .map(|v| Vec::<String>::try_from(v).expect("uris is an array of strings"))
            .unwrap_or_default()
    }

    fn with_option(key: &str, value: bool) -> HashMap<String, OwnedValue> {
        let mut options = HashMap::new();
        options.insert(key.to_string(), OwnedValue::from(value));
        options
    }

    // --- decide(): pure guard proofs, no bus involved. ---

    #[test]
    fn decide_success_one_local_path_returns_response_zero_and_one_uri() {
        let fixture = ScratchFile::create();
        let expected_uri = gio::File::for_path(fixture.path()).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_SUCCESS);
        let uris = uris_of(&results);
        assert_eq!(uris, vec![expected_uri]);
        fixture.remove();
    }

    #[test]
    fn decide_cancelled_returns_response_one_and_no_uri() {
        let (response, results) = decide(&HashMap::new(), |_| ChooserOutcome::Cancelled.into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_CANCELLED);
        assert!(results.is_empty());
    }

    #[test]
    fn decide_failed_returns_response_other_and_no_uri() {
        let (response, results) = decide(&HashMap::new(), |_| ChooserOutcome::Failed.into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
        assert!(results.is_empty());
    }

    macro_rules! guard_proof {
        ($red:ident, $bypassed:ident, $options:expr, $override:expr) => {
            #[test]
            fn $red() {
                let fixture = ScratchFile::create();
                let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
                let (response, _) = decide(&$options, |_| outcome.clone().into(), GuardOverride::None);
                assert_eq!(response, RESPONSE_OTHER);
                fixture.remove();
            }

            #[test]
            fn $bypassed() {
                let fixture = ScratchFile::create();
                let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
                let (response, _) = decide(&$options, |_| outcome.clone().into(), $override);
                assert_eq!(
                    response, RESPONSE_SUCCESS,
                    "bypassing this guard must turn the request's own refusal into success"
                );
                fixture.remove();
            }
        };
    }

    guard_proof!(
        decide_refuses_directory_true,
        decide_bypassing_directory_guard_then_succeeds,
        with_option("directory", true),
        GuardOverride::SkipDirectory
    );

    /// [D-222 replacement for `decide_refuses_multiple_true` /
    /// `decide_bypassing_multiple_guard_then_succeeds`]: `multiple` is no
    /// longer refused outright -- a single real picked path still succeeds
    /// as a one-element `uris` list even when the caller asked for
    /// `multiple=true`.
    #[test]
    fn decide_honors_multiple_true_with_one_picked_path() {
        let fixture = ScratchFile::create();
        let expected_uri = gio::File::for_path(fixture.path()).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(
            &with_option("multiple", true),
            |_| outcome.clone().into(),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec![expected_uri]);
        fixture.remove();
    }

    /// [D-222 replacement for `decide_refuses_filters_present` /
    /// `decide_bypassing_filters_guard_then_succeeds`]: a well-formed
    /// `filters` value is decoded and honored, not refused.
    #[test]
    fn decide_honors_filters_present_well_formed() {
        let fixture = ScratchFile::create();
        let mut options = HashMap::new();
        options.insert(
            "filters".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "Text".to_string(),
                vec![(0u32, "*.txt".to_string())],
            )]))
            .expect("filters encodes"),
        );
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(&options, |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert!(!uris_of(&results).is_empty());
        fixture.remove();
    }

    /// [D-222 replacement for `decide_refuses_current_filter_present` /
    /// `decide_bypassing_current_filter_guard_then_succeeds`]: a
    /// `current_filter` matching a supplied `filters` entry resolves to
    /// that same entry in `results.current_filter`, not a refusal.
    #[test]
    fn decide_honors_matching_current_filter() {
        let fixture = ScratchFile::create();
        let filter_value = (
            "Text".to_string(),
            vec![(0u32, "*.txt".to_string())],
        );
        let mut options = HashMap::new();
        options.insert(
            "filters".to_string(),
            OwnedValue::try_from(Value::from(vec![filter_value.clone()])).expect("filters encodes"),
        );
        options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(filter_value)).expect("current_filter encodes"),
        );
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(&options, |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert!(results.contains_key("current_filter"));
        fixture.remove();
    }

    /// [D-222 replacement for `decide_refuses_choices_present` /
    /// `decide_bypassing_choices_guard_then_succeeds`]: a well-formed
    /// `choices` value is decoded, its default resolved, and returned in
    /// `results.choices`, not refused.
    #[test]
    fn decide_honors_choices_present_well_formed() {
        let fixture = ScratchFile::create();
        let mut options = HashMap::new();
        options.insert(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "readonly".to_string(),
                "Read only".to_string(),
                Vec::<(String, String)>::new(),
                String::new(),
            )]))
            .expect("choices encodes"),
        );
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(&options, |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert!(results.contains_key("choices"));
        fixture.remove();
    }

    /// [D-222]: a malformed `filters` value (wrong inner shape) still
    /// fails closed with a plain refusal, never a panic, and never
    /// reaching the chooser.
    #[test]
    fn decide_refuses_malformed_filters_without_panic() {
        let mut options = HashMap::new();
        options.insert("filters".to_string(), OwnedValue::from(7u32));
        let (response, _) = decide(
            &options,
            |_| panic!("a malformed filters value must never reach the chooser"),
            GuardOverride::None,
        );
        assert_eq!(response, RESPONSE_OTHER);
    }

    #[test]
    fn decide_refuses_more_than_one_picked_path() {
        let fixture = ScratchFile::create();
        let outcome = ChooserOutcome::Picked(vec![
            fixture.path().to_path_buf(),
            PathBuf::from("/nonexistent-round212-path/second.txt"),
        ]);
        let (response, _) = decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
        fixture.remove();
    }

    #[test]
    fn decide_bypassing_cardinality_guard_then_succeeds_on_two_paths() {
        let fixture = ScratchFile::create();
        let expected_uri = gio::File::for_path(fixture.path()).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![
            fixture.path().to_path_buf(),
            PathBuf::from("/nonexistent-round212-path/second.txt"),
        ]);
        let (response, results) = decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::SkipCardinality);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec![expected_uri], "only the first path is normalized");
        fixture.remove();
    }

    #[test]
    fn decide_refuses_non_local_output() {
        let outcome = ChooserOutcome::Picked(vec![PathBuf::from("relative/not-absolute.txt")]);
        let (response, _) = decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
    }

    #[test]
    fn decide_bypassing_invalid_output_guard_then_succeeds_on_non_local_path() {
        let outcome = ChooserOutcome::Picked(vec![PathBuf::from("relative/not-absolute.txt")]);
        let (response, results) =
            decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::SkipInvalidOutput);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec!["file:///forced-bypass-proof".to_string()]);
    }

    #[test]
    fn decide_refuses_missing_absolute_path() {
        let outcome =
            ChooserOutcome::Picked(vec![PathBuf::from("/nonexistent-round212-path/missing.txt")]);
        let (response, results) = decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
    }

    #[test]
    fn decide_bypassing_missing_path_guard_then_succeeds_on_missing_file() {
        let missing = PathBuf::from("/nonexistent-round212-path/missing.txt");
        let expected_uri = gio::File::for_path(&missing).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![missing]);
        let (response, results) =
            decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::SkipMissingPath);
        assert_eq!(
            response, RESPONSE_SUCCESS,
            "bypassing only the missing-path guard must turn this same request's refusal into success"
        );
        assert_eq!(uris_of(&results), vec![expected_uri]);
    }

    #[test]
    fn decide_bypassing_missing_path_guard_still_refuses_non_local_path() {
        let outcome = ChooserOutcome::Picked(vec![PathBuf::from("relative/not-absolute.txt")]);
        let (response, results) =
            decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::SkipMissingPath);
        assert_eq!(
            response, RESPONSE_OTHER,
            "a relative candidate is never caught by the missing-path guard, so waiving it must not make this succeed"
        );
        assert!(uris_of(&results).is_empty());
    }

    #[test]
    fn decide_bypassing_invalid_output_guard_still_refuses_missing_absolute_path() {
        let outcome =
            ChooserOutcome::Picked(vec![PathBuf::from("/nonexistent-round212-path/missing.txt")]);
        let (response, results) =
            decide(&HashMap::new(), |_| outcome.clone().into(), GuardOverride::SkipInvalidOutput);
        assert_eq!(
            response, RESPONSE_OTHER,
            "waiving only invalid-output normalization must not also waive the missing-path guard"
        );
        assert!(uris_of(&results).is_empty());
    }

    // --- Real D-Bus calls on a private session bus. ---

    #[test]
    fn dbus_openfile_success_returns_one_normalized_uri() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let fixture = bus.create_fixture_file("picked.txt");
        let expected_uri = gio::File::for_path(&fixture).uri().to_string();
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![fixture])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec![expected_uri]);
        assert_eq!(chooser.call_count(), 1);
        finish_private_bus(bus, server, client);
    }

    #[test]
    fn dbus_openfile_missing_path_returns_response_other_no_uri() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let missing = bus.root().join("never-created.txt");
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![missing])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        assert_eq!(chooser.call_count(), 1);
        finish_private_bus(bus, server, client);
    }

    #[test]
    fn dbus_openfile_cancel_returns_response_one_no_uri() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Cancelled]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_CANCELLED);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
    }

    #[test]
    fn dbus_openfile_failure_returns_response_other_no_uri() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Failed]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
    }

    // --- Exported `OpenFile`, through the real `ProcessChooser` and a
    // real `FakeNative` child, on the private session bus: the full
    // failure matrix taken all the way through the D-Bus boundary, not
    // just through `pick_single`/`decide` directly.

    #[test]
    fn dbus_openfile_process_chooser_signal_death_fails_closed() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("kill -9 $$\n");
        let base = scratch_base("dbus-signal");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_deadline_expiry_fails_closed() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("sleep 5\nexit 0\n");
        let base = scratch_base("dbus-deadline");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_millis(200),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        assert_eq!(count_fake_native_processes(native.path()), 0);
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_no_write_fails_closed_not_cancelled() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("exit 1\n");
        let base = scratch_base("dbus-no-write");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(
            response, RESPONSE_OTHER,
            "exit 1 with no result file written is not a confirmed cancel"
        );
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_malformed_result_fails_closed() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("printf '%s' '/some/path' > \"$3\"\nexit 0\n");
        let base = scratch_base("dbus-malformed");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_multiple_paths_refused() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("printf 'a\\nb\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("dbus-multi");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_relative_output_refused() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted("printf 'relative/path.txt\\n' > \"$3\"\nexit 0\n");
        let base = scratch_base("dbus-relative");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_missing_path_refused() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let native = FakeNative::scripted(
            "printf '/nonexistent-d205-dbus-path/missing.txt\\n' > \"$3\"\nexit 0\n",
        );
        let base = scratch_base("dbus-missing");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
    }

    #[test]
    fn dbus_openfile_process_chooser_directory_output_refused() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let dir_fixture = scratch_base("dbus-dir-fixture");
        fs::create_dir_all(&dir_fixture).expect("create directory fixture");
        let native = FakeNative::scripted(&format!(
            "printf '%s\\n' '{}' > \"$3\"\nexit 0\n",
            dir_fixture.display()
        ));
        let base = scratch_base("dbus-dir");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
        finish_private_bus(bus, server, client);
        finish_base(base);
        fs::remove_dir_all(&dir_fixture).expect("remove exact directory-output fixture");
    }

    #[test]
    fn dbus_openfile_option_refusal_never_launches_process_chooser_child() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let counter_dir = scratch_base("dbus-launch-marker");
        fs::create_dir_all(&counter_dir).expect("create counter dir");
        let marker = counter_dir.join("launches");
        let native = FakeNative::scripted(&format!(
            ": > \"$3\"\nprintf 'x' >> '{}'\nexit 1\n",
            marker.display()
        ));
        let base = scratch_base("dbus-launch-free");
        let chooser: Arc<dyn Chooser> = Arc::new(ProcessChooser::for_test(
            native.path().to_path_buf(),
            Duration::from_secs(5),
            base.clone(),
        ));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        // D-222 moved `multiple` from a pre-pick refusal to an honored
        // option, so this proof is now keyed on `directory=true`, the one
        // pre-pick refusal this round leaves unchanged.
        let (response, _) = call_open_file(&client, with_option("directory", true)).expect("OpenFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(
            !marker.exists(),
            "an option refusal must never reach the chooser seam, so the child must never launch"
        );
        assert!(
            !base.exists(),
            "an option refusal must never reach the chooser seam, so the scratch base must never be created either"
        );
        finish_private_bus(bus, server, client);
        fs::remove_dir_all(&counter_dir).expect("remove exact launch-marker fixture");
    }

    macro_rules! dbus_option_refusal {
        ($name:ident, $options:expr) => {
            #[test]
            fn $name() {
                let _guard = env_lock();
                let bus = PrivateBus::start();
                let fixture = bus.create_fixture_file("should-not-be-read.txt");
                let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![fixture])]);
                let server = bus.connect_server(chooser.clone());
                let client = bus.connect_client();

                let (response, _) = call_open_file(&client, $options).expect("OpenFile call");
                assert_eq!(response, RESPONSE_OTHER);
                assert_eq!(
                    chooser.call_count(),
                    0,
                    "this option must refuse before the chooser seam is ever invoked"
                );
                finish_private_bus(bus, server, client);
            }
        };
    }

    dbus_option_refusal!(dbus_openfile_refuses_directory_true, with_option("directory", true));

    /// [D-222 replacement for `dbus_openfile_refuses_multiple_true`]:
    /// `multiple=true` over the real D-Bus boundary now reaches the
    /// chooser and succeeds, rather than being refused before it.
    #[test]
    fn dbus_openfile_honors_multiple_true() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let fixture = bus.create_fixture_file("multiple-honored.txt");
        let expected_uri = gio::File::for_path(&fixture).uri().to_string();
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![fixture])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) =
            call_open_file(&client, with_option("multiple", true)).expect("OpenFile call");
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec![expected_uri]);
        assert_eq!(chooser.call_count(), 1);
        finish_private_bus(bus, server, client);
    }

    /// [D-222 replacement for `dbus_openfile_refuses_filters_present`,
    /// `dbus_openfile_refuses_current_filter_present` and
    /// `dbus_openfile_refuses_choices_present`]: a well-formed
    /// `filters`/`current_filter`/`choices` request over the real D-Bus
    /// boundary now reaches the chooser and succeeds, with the resolved
    /// filter and choices coming back in the results.
    #[test]
    fn dbus_openfile_honors_filters_current_filter_and_choices() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let fixture = bus.create_fixture_file("filters-honored.txt");
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![fixture])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) =
            call_open_file(&client, browser_shaped_options()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(chooser.call_count(), 1);
        assert!(results.contains_key("current_filter"));
        assert!(results.contains_key("choices"));
        finish_private_bus(bus, server, client);
    }

    /// He hit this one for real: "Save image as" in Chrome did nothing at
    /// all, because registering the FileChooser backend claims the WHOLE
    /// interface and only `OpenFile` was exported, so every Save request
    /// went unanswered with no dialog and no error. This proves the method
    /// is now there and answers over the real bus.
    #[test]
    fn dbus_savefile_answers_over_the_real_bus() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let target = bus.root.join("saved-here.png");
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![target.clone()])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let mut options = HashMap::new();
        options.insert(
            "current_name".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from("saved-here.png")),
        );
        let (response, results) = call_save_file(&client, options).expect("SaveFile call");
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(chooser.call_count(), 1);
        assert_eq!(
            uris_of(&results),
            vec![format!("file://{}", target.display())]
        );
        finish_private_bus(bus, server, client);
    }

    /// A Save result whose folder does not exist is refused: the caller
    /// would be handed a URI it cannot write, and would fail after the
    /// dialog had already closed.
    #[test]
    fn dbus_savefile_refuses_a_target_in_a_folder_that_does_not_exist() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let target = bus.root.join("no-such-folder").join("file.png");
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![target])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (response, results) = call_save_file(&client, HashMap::new()).expect("SaveFile call");
        assert_eq!(response, RESPONSE_OTHER);
        assert!(results.is_empty());
        assert_eq!(chooser.call_count(), 1);
        finish_private_bus(bus, server, client);
    }

    /// Unlike an Open, a Save target is allowed NOT to exist yet -- that is
    /// the normal case. The missing-path guard must therefore never run for
    /// a Save, which the success test above covers, and the writability
    /// guard must be the only thing that can refuse one. `SkipSaveTarget`
    /// disables exactly that guard and nothing else, which proves the
    /// refusal above came from it.
    #[test]
    fn save_target_guard_is_the_only_thing_refusing_a_save_result() {
        let candidate = PathBuf::from("/nonexistent-wayfinder-save-proof/file.png");
        let options: HashMap<String, OwnedValue> = HashMap::new();
        let outcome = || PickOutcome::from(ChooserOutcome::Picked(vec![candidate.clone()]));

        let (guarded, _) = decide_with_shape(
            &options,
            |_| outcome(),
            GuardOverride::None,
            RequestShape::Save,
        );
        assert_eq!(guarded, RESPONSE_OTHER, "the guard must refuse this result");

        let (waived, results) = decide_with_shape(
            &options,
            |_| outcome(),
            GuardOverride::SkipSaveTarget,
            RequestShape::Save,
        );
        assert_eq!(
            waived, RESPONSE_SUCCESS,
            "waiving only the save-target guard must let the same result through"
        );
        assert_eq!(
            uris_of(&results),
            vec![format!("file://{}", candidate.display())]
        );
    }

    #[test]
    fn dbus_savefiles_is_an_unknown_method() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let chooser = ScriptedChooser::new(vec![]);
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let err = client
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE_NAME),
                "SaveFiles",
                &(),
            )
            .expect_err("SaveFiles is never exported");
        match err {
            zbus::Error::MethodError(name, ..) => {
                assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.UnknownMethod");
            }
            other => panic!("expected UnknownMethod, got {other:?}"),
        }
        finish_private_bus(bus, server, client);
    }

    #[test]
    fn dbus_introspection_proves_bus_name_path_interface_and_signature() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let chooser = ScriptedChooser::new(vec![]);
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let message = client
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            )
            .expect("Introspect call");
        let xml: String = message.body().deserialize().expect("introspection body is a string");

        assert!(xml.contains(INTERFACE_NAME));
        assert!(xml.contains("OpenFile"));
        assert!(xml.contains("SaveFile\""), "SaveFile must be exported");
        // OpenFile's exact signature: handle(o), app_id(s), parent_window(s),
        // title(s), options(a{sv}) in; response(u), results(a{sv}) out.
        let open_file_block = xml
            .split("<method name=\"OpenFile\">")
            .nth(1)
            .and_then(|rest| rest.split("</method>").next())
            .expect("OpenFile method block present");
        assert_eq!(open_file_block.matches("type=\"o\"").count(), 1);
        assert_eq!(open_file_block.matches("type=\"s\"").count(), 3);
        assert_eq!(open_file_block.matches("type=\"a{sv}\"").count(), 2);
        assert_eq!(open_file_block.matches("type=\"u\"").count(), 1);
        finish_private_bus(bus, server, client);
    }

    #[test]
    fn dbus_second_sequential_call_is_independent_no_state_leak() {
        let _guard = env_lock();
        let bus = PrivateBus::start();
        let fixture = bus.create_fixture_file("first.txt");
        let expected_uri = gio::File::for_path(&fixture).uri().to_string();
        let chooser = ScriptedChooser::new(vec![
            ChooserOutcome::Picked(vec![fixture]),
            ChooserOutcome::Cancelled,
        ]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();

        let (first_response, first_results) =
            call_open_file(&client, HashMap::new()).expect("first OpenFile call");
        assert_eq!(first_response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&first_results), vec![expected_uri]);

        let (second_response, second_results) =
            call_open_file(&client, HashMap::new()).expect("second OpenFile call");
        assert_eq!(second_response, RESPONSE_CANCELLED);
        assert!(uris_of(&second_results).is_empty());
        assert_eq!(chooser.call_count(), 2);
        finish_private_bus(bus, server, client);
    }

    /// D-236, and he hit it: "upload files button still doesnt open
    /// wayfinder, if it does it's not doing it where it should". The picker
    /// registered the SAME GApplication id as the ordinary file manager, so
    /// whenever Wayfinder was already running the picker process became a
    /// REMOTE instance: GApplication forwarded `activate` to the running
    /// window, raised it wherever it happened to be, and the picker exited
    /// without ever answering the request.
    ///
    /// This reproduces that condition exactly -- a real primary Wayfinder
    /// already running on the same bus and display -- and requires the
    /// request to still be answered by the picker's own window.
    #[test]
    fn a_running_wayfinder_does_not_swallow_the_picker_d236() {
        let _guard = env_lock();
        let (driver, _driver_target) = build_picker_test_driver("picker-test-driver-pick");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("d236-base");
        let fixture_dir_root = OwnedScratchDir::create(scratch_base("d236-fixture-home"));
        let fixture_file = fixture_dir_root.path().join("d236-fixture.txt");
        fs::write(&fixture_file, b"d236 fixture\n").expect("write fixture file");

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        // The ordinary file manager, already running, on this bus and this
        // display. This is the whole point of the test.
        let mut primary = Command::new(&driver)
            .env("DISPLAY", &xvfb.display)
            .env("HOME", fixture_dir_root.path())
            .spawn()
            .expect("spawn an ordinary Wayfinder");
        // Long enough for it to own its application id on this bus.
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            primary.try_wait().expect("poll the primary").is_none(),
            "the primary Wayfinder must still be running for this test to mean anything"
        );

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver.clone(), Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let call = call_open_file(&client, HashMap::new());

        let _ = primary.kill();
        let _ = primary.wait();
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }

        let (response, results) = call.expect("OpenFile call");
        let expected_uri = gio::File::for_path(&fixture_file).uri().to_string();
        assert_eq!(
            response, RESPONSE_SUCCESS,
            "a request made while Wayfinder is running must still be answered by the picker"
        );
        assert_eq!(uris_of(&results), vec![expected_uri]);

        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
    }


    #[test]
    fn dbus_no_desktop_oracle_runs_without_display_wayland_or_gdk_backend() {
        let _guard = env_lock();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        let old_gdk_backend = std::env::var("GDK_BACKEND").ok();
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("GDK_BACKEND");

        let before = count_desktop_processes();

        let bus = PrivateBus::start();
        let fixture = bus.create_fixture_file("no-desktop.txt");
        let chooser = ScriptedChooser::new(vec![ChooserOutcome::Picked(vec![fixture])]);
        let server = bus.connect_server(chooser.clone());
        let client = bus.connect_client();
        let (response, _) = call_open_file(&client, HashMap::new()).expect("OpenFile call");
        assert_eq!(response, RESPONSE_SUCCESS);

        let after = count_desktop_processes();
        assert_eq!(
            before, after,
            "no wayfinder-native, GTK window or xdg-desktop-portal process may start"
        );

        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        match old_gdk_backend {
            Some(v) => std::env::set_var("GDK_BACKEND", v),
            None => std::env::remove_var("GDK_BACKEND"),
        }
    }

    fn count_desktop_processes() -> usize {
        let output = Command::new("pgrep")
            .arg("-f")
            .arg("wayfinder-native|xdg-desktop-portal")
            .output()
            .expect("run pgrep");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    }

    // --- Real native picker proof: no `FakeNative`, the actual
    // `wayfinder-native` binary built with the test-only `picker-test-driver`
    // feature, under a private Xvfb display and the private session bus. ---

    /// A private, round-owned X server. Never the owner's real display --
    /// its own numbered socket under `/tmp/.X11-unix/`, killed and reaped on
    /// drop.
    /// A private, round-owned Xvfb. Ruled by the Coordinator (round 214):
    /// Xvfb's own `/tmp/.X11-unix/X*` socket and `/tmp/.X*-lock` file are
    /// outside the `/tmp` ban -- nothing alternative is installed for
    /// this -- but the display number must be chosen so it cannot collide
    /// with Zito's live session, and the server must be reaped (and its
    /// own lock file gone) on success, failure, panic and timeout alike.
    struct PrivateXvfb {
        display: String,
        display_num: u32,
        child: Child,
        /// Set once `finish()` has explicitly reaped the child and
        /// checked its lock file, so `Drop` never attempts a second,
        /// unchecked reap of a pid that may since have been reused.
        reaped: Cell<bool>,
    }

    impl PrivateXvfb {
        /// High, round-owned display range, well above any display a
        /// live interactive session would use, so a collision with
        /// Zito's own session is not plausible.
        fn pick_display_num() -> u32 {
            60000 + (next_unique_id() % 1000) as u32
        }

        fn lock_path_for(display_num: u32) -> PathBuf {
            PathBuf::from(format!("/tmp/.X{display_num}-lock"))
        }

        fn socket_path_for(display_num: u32) -> PathBuf {
            PathBuf::from("/tmp/.X11-unix").join(format!("X{display_num}"))
        }

        fn start() -> Self {
            let mut display_num = Self::pick_display_num();
            while Self::lock_path_for(display_num).exists() {
                display_num = Self::pick_display_num();
            }
            let display = format!(":{display_num}");
            let child = Command::new("Xvfb")
                .arg(&display)
                .arg("-screen")
                .arg("0")
                .arg("1280x1024x24")
                .arg("-nolisten")
                .arg("tcp")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn a private Xvfb");
            let socket = Self::socket_path_for(display_num);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !socket.exists() {
                assert!(Instant::now() < deadline, "private Xvfb never created its socket");
                std::thread::sleep(Duration::from_millis(20));
            }
            Self {
                display,
                display_num,
                child,
                reaped: Cell::new(false),
            }
        }

        /// Signals the exact child to exit cleanly (so it tears down its
        /// own lock file instead of leaving one behind for a SIGKILL),
        /// waits for it, falls back to SIGKILL only if it does not exit
        /// in time, then asserts the exact reap actually happened and
        /// its lock file is gone. Checked: a surviving process or a
        /// surviving lock file fails the test rather than being ignored.
        fn finish(mut self) {
            let pid = self.child.id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match self.child.try_wait().expect("poll the private Xvfb child") {
                    Some(_) => break,
                    None if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    None => {
                        let _ = self.child.kill();
                        self.child.wait().expect("reap private Xvfb after SIGKILL fallback");
                        break;
                    }
                }
            }
            self.reaped.set(true);
            assert!(
                !PathBuf::from(format!("/proc/{pid}")).exists(),
                "private Xvfb process must be gone after finish()"
            );
            assert!(
                !Self::lock_path_for(self.display_num).exists(),
                "private Xvfb's own lock file must not survive the reap"
            );
            assert!(
                !Self::socket_path_for(self.display_num).exists(),
                "private Xvfb's own socket must not survive the reap"
            );
        }
    }

    /// Unwind-safe fallback: a bounded best-effort SIGTERM-then-SIGKILL
    /// reap, never asserting anything (so it can never panic during an
    /// unwind), and skipped entirely when `finish()` already reaped it.
    impl Drop for PrivateXvfb {
        fn drop(&mut self) {
            if self.reaped.get() {
                return;
            }
            let pid = self.child.id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            for _ in 0..50 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// An owned round-owned scratch directory (a driver build's own
    /// target dir, or a fixture `HOME`) with checked normal `finish()`
    /// and a bounded, non-panicking unwind fallback -- the same shape as
    /// `ScratchFile`/`PrivateBus`, generalized so the two real-native-
    /// picker tests do not leak `driver_target`/`fixture_dir_root` on an
    /// assertion failure partway through the test body.
    struct OwnedScratchDir {
        dir: PathBuf,
        removed: Cell<bool>,
    }

    impl OwnedScratchDir {
        /// D-208: the owner exists -- and its unwind-safe `Drop` fallback
        /// is live -- before the directory is created, not after. Callers
        /// that build a round-owned root must use this instead of calling
        /// `fs::create_dir_all` and then `adopt`, which leaves the interval
        /// between the two calls unowned.
        fn create(dir: PathBuf) -> Self {
            let owner = Self {
                dir,
                removed: Cell::new(false),
            };
            fs::create_dir_all(owner.path()).expect("create owned scratch dir");
            owner
        }

        fn path(&self) -> &Path {
            &self.dir
        }

        /// D-208 test seam: lets a test observe this still-live owner's
        /// exact root before removal. Unlike `mem::forget`, this never
        /// gives up ownership, so a panic, timeout or other unwind during
        /// `f` still passes through `Drop`'s fallback removal.
        fn peek_while_live<T>(&self, f: impl FnOnce() -> T) -> T {
            f()
        }

        /// Explicitly, observably removes the exact dir. Failure fails
        /// the test; `Drop`'s fallback removal is skipped afterward.
        fn finish(self) {
            fs::remove_dir_all(&self.dir).expect("remove the exact owned scratch dir");
            self.removed.set(true);
        }
    }

    impl Drop for OwnedScratchDir {
        fn drop(&mut self) {
            if !self.removed.get() {
                let _ = fs::remove_dir_all(&self.dir);
            }
        }
    }

    /// Builds the test-only native picker driver artifact for one feature
    /// (`picker-test-driver-pick` or `picker-test-driver-cancel`) into a
    /// round-owned target dir below `~/Projects/Scratch/`, never into the
    /// shared project `target/`. Returns the built binary's exact path.
    /// This feature does not exist in the default/shipped `wayfinder-native`
    /// build -- see `Cargo.toml`.
    fn build_picker_test_driver(feature: &str) -> (PathBuf, OwnedScratchDir) {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let target_dir = scratch_base(&format!("native-driver-target-{feature}"));
        let target_dir = OwnedScratchDir::create(target_dir);
        let status = Command::new("cargo")
            .arg("build")
            .arg("--quiet")
            .arg("--bin")
            .arg("wayfinder-native")
            .arg("--features")
            .arg(feature)
            .arg("--target-dir")
            .arg(target_dir.path())
            .current_dir(manifest_dir)
            .status()
            .expect("run cargo build for the test-only native driver");
        assert!(status.success(), "building the {feature} native driver failed");
        let binary = target_dir.path().join("debug").join("wayfinder-native");
        assert!(binary.exists(), "expected driver binary at {}", binary.display());
        (binary, target_dir)
    }

    /// The real `wayfinder-native` binary, launched by the real portal, as
    /// the answer to a real `SaveFile` call over the real bus: the name box
    /// starts from the request's own `current_name`, the real Save button
    /// callback runs, and the URI that comes back is that name inside the
    /// folder the request asked for. This is the proof that "Save image as"
    /// now reaches Wayfinder instead of doing nothing.
    #[test]
    fn native_picker_real_save_through_exported_save_file() {
        let _guard = env_lock();
        let (driver, _driver_target) = build_picker_test_driver("picker-test-driver-save");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("native-save-base");
        let fixture_dir_root = OwnedScratchDir::create(scratch_base("native-save-fixture-home"));
        // One ordinary file in the folder, so the listing is non-empty and
        // the driver has something it could have selected: the name that
        // comes back must still be the typed one, not this.
        fs::write(
            fixture_dir_root.path().join("already-here.txt"),
            b"not the save target\n",
        )
        .expect("write fixture file");
        let expected_file = fixture_dir_root.path().join("saved-by-the-driver.png");

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver, Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let mut options = HashMap::new();
        options.insert(
            "current_name".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from("saved-by-the-driver.png")),
        );
        options.insert(
            "current_folder".to_string(),
            OwnedValue::try_from(Value::from(
                fixture_dir_root
                    .path()
                    .to_string_lossy()
                    .as_bytes()
                    .iter()
                    .copied()
                    .chain(std::iter::once(0u8))
                    .collect::<Vec<u8>>(),
            ))
            .expect("current_folder encodes"),
        );
        let (response, results) = call_save_file(&client, options).expect("SaveFile call");

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let expected_uri = gio::File::for_path(&expected_file).uri().to_string();
        assert_eq!(response, RESPONSE_SUCCESS, "a real Save must report success");
        assert_eq!(
            uris_of(&results),
            vec![expected_uri],
            "the exported URI must be the typed name inside the requested folder"
        );
        assert!(
            !expected_file.exists(),
            "the picker names the target, it must never create it"
        );

        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be gone after a completed real save"
        );
    }

    #[test]
    fn native_picker_real_pick_through_exported_open_file() {
        let _guard = env_lock();
        let (driver, driver_target) = build_picker_test_driver("picker-test-driver-pick");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        // Both of these read the real `$HOME` to build their own scratch
        // roots, so they must run before `$HOME` is overridden below.
        let bus = PrivateBus::start();
        let base = scratch_base("native-pick-base");
        let fixture_dir_root = OwnedScratchDir::create(scratch_base("native-pick-fixture-home"));
        let fixture_file = fixture_dir_root.path().join("d205-native-picker-fixture.txt");
        fs::write(&fixture_file, b"d205 round 214 real-picker fixture\n").expect("write fixture file");

        // The real app's picker has no `--starting-dir` on this call (the
        // production spawn line is exactly `--pick --out <path>`, unchanged
        // by this test), so it falls back to its own "Home" standard
        // directory, which it reads from `$HOME`. Pointing `$HOME` at the
        // one-file fixture -- not the owner's real home -- is what keeps the
        // real driver from ever listing or selecting a real owner path.
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver, Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let expected_uri = gio::File::for_path(&fixture_file).uri().to_string();
        assert_eq!(response, RESPONSE_SUCCESS, "real pick must report success");
        assert_eq!(
            uris_of(&results),
            vec![expected_uri],
            "the exported URI must be the real driver's exact selected fixture"
        );

        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be gone after a completed real pick"
        );
        finish_base(base);
        driver_target.finish();
    }

    #[test]
    fn native_picker_real_cancel_through_exported_open_file() {
        let _guard = env_lock();
        let (driver, driver_target) = build_picker_test_driver("picker-test-driver-cancel");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("native-cancel-base");
        let fixture_dir_root = OwnedScratchDir::create(scratch_base("native-cancel-fixture-home"));
        fs::write(
            fixture_dir_root.path().join("d205-native-picker-fixture.txt"),
            b"d205 round 214 real-picker fixture\n",
        )
        .expect("write fixture file");

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver, Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let (response, results) = call_open_file(&client, HashMap::new()).expect("OpenFile call");

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        assert_eq!(response, RESPONSE_CANCELLED, "real Cancel must report cancelled");
        assert!(uris_of(&results).is_empty(), "a real Cancel must carry no uri");

        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be gone after a completed real cancel"
        );
        finish_base(base);
        driver_target.finish();
    }

    /// Browser-shaped `filters` value: `a(sa(us))`, one named filter per
    /// entry, each rule a `(type, pattern)` pair (`0` = glob, `1` = mime).
    /// Three entries, not two: the initial (`current_filter`) entry here
    /// must admit a *different* fixture than the one the driver cycles
    /// to, or a regression that ignores filtering entirely and a
    /// regression that filters correctly produce the same picked set --
    /// see round 223.
    fn browser_filters() -> Vec<(String, Vec<(u32, String)>)> {
        vec![
            ("Markdown files".to_string(), vec![(0u32, "*.md".to_string())]),
            ("Text files".to_string(), vec![(0u32, "*.txt".to_string())]),
            ("All files".to_string(), vec![(0u32, "*".to_string())]),
        ]
    }

    /// Browser-shaped `current_filter` value: `(sa(us))`, exactly matching
    /// the first entry `browser_filters` returns.
    fn browser_current_filter() -> (String, Vec<(u32, String)>) {
        browser_filters().into_iter().next().expect("at least one filter")
    }

    /// Browser-shaped `choices` value: `a(ssa(ss)s)` -- one enumerated
    /// choice (nonempty option list) and one boolean choice (empty option
    /// list, initial `"false"`).
    fn browser_choices() -> Vec<(String, String, Vec<(String, String)>, String)> {
        vec![
            (
                "encoding".to_string(),
                "Encoding".to_string(),
                vec![
                    ("utf8".to_string(), "UTF-8".to_string()),
                    ("latin1".to_string(), "Latin-1".to_string()),
                ],
                "utf8".to_string(),
            ),
            (
                "readonly".to_string(),
                "Read only".to_string(),
                Vec::new(),
                "false".to_string(),
            ),
        ]
    }

    fn browser_shaped_options() -> HashMap<String, OwnedValue> {
        let mut options = HashMap::new();
        options.insert("multiple".to_string(), OwnedValue::from(true));
        options.insert(
            "filters".to_string(),
            OwnedValue::try_from(Value::from(browser_filters())).expect("filters encodes"),
        );
        options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(browser_current_filter()))
                .expect("current_filter encodes"),
        );
        options.insert(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(browser_choices())).expect("choices encodes"),
        );
        options
    }

    /// Admitted only when "Markdown files" (`*.md`, supported by every
    /// glob implementation ever shipped) is the *active* filter.
    fn markdown_filter() -> (String, Vec<(u32, String)>) {
        ("Markdown files".to_string(), vec![(0u32, "*.md".to_string())])
    }

    /// Admitted only when "Text files (strict case)" is active, using the
    /// installed portal contract's bracket character-class form
    /// (`*.[iI][cC][oO]` is the installed example this mirrors). Committed
    /// `75ca650`'s `glob_match` supports only `*`/`?` and treats `[`/`]` as
    /// literal characters, so this pattern matches no real filename under
    /// it -- a truthful red against the rejected implementation, not a
    /// synthetic forced failure. Round 224 must make this pattern work.
    fn charclass_text_filter() -> (String, Vec<(u32, String)>) {
        (
            "Text files (strict case)".to_string(),
            vec![(0u32, "*.[tT][xX][tT]".to_string())],
        )
    }

    /// Builds one `OpenFile` options map for the real-child regression:
    /// `multiple`, the two filters above in the given order (so the test
    /// controls which one the driver's forward-by-one cycle lands on),
    /// `current_filter` matching `filters[0]`, and the shared `choices`.
    fn real_child_filter_options(
        filters: Vec<(String, Vec<(u32, String)>)>,
    ) -> HashMap<String, OwnedValue> {
        let current_filter = filters.first().cloned().expect("at least one filter");
        let mut options = HashMap::new();
        options.insert("multiple".to_string(), OwnedValue::from(true));
        options.insert(
            "filters".to_string(),
            OwnedValue::try_from(Value::from(filters)).expect("filters encodes"),
        );
        options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(current_filter)).expect("current_filter encodes"),
        );
        options.insert(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(browser_choices())).expect("choices encodes"),
        );
        options
    }

    /// [D-222/223]: the real-child filter regression, corrected per round
    /// 223 to be able to fail. Exercises the real native picker driver
    /// binary (`picker-test-driver-pick` feature), the real filter
    /// dropdown and both real choice controls, over a private D-Bus
    /// `OpenFile` call backed by the real `ProcessChooser` -- proving the
    /// whole wire protocol round-trips through actual GTK controls, not
    /// just `decide()`'s own `ScriptedChooser` path (see
    /// `dbus_openfile_honors_multiple_true` and
    /// `dbus_openfile_honors_filters_current_filter_and_choices`, which
    /// only prove that narrower layer).
    ///
    /// Round 222's shape of this test cycled the dropdown *after*
    /// selecting, with three fixtures that happened to sort so the two
    /// lexically-first eligible rows were always the two intended
    /// "matching" files regardless of whether filtering ran at all -- a
    /// false green. `native.rs`'s driver now cycles before selecting (see
    /// `schedule_picker_test_driver`), which fixes the ordering half of
    /// the defect but is not sufficient alone: with only an ordinary
    /// `*.txt` second filter, `75ca650`'s real (if falsely-ordered->now
    /// correctly-ordered) matcher already implements that pattern, so the
    /// test would pass today regardless of whether round-222's defect was
    /// ever present. Round 223 makes the *second* filter exercise a
    /// pattern shape (`*.[tT][xX][tT]`) that `75ca650` cannot match at
    /// all, and makes the real child run twice in the same test with the
    /// two filters reordered between runs, so each fixture's admission
    /// under one filter and refusal under the other is proven by the real
    /// native process itself in the same test, not by a suffix check on a
    /// filename or a direct call into a copied matcher.
    #[test]
    fn dbus_openfile_browser_shaped_multi_filter_choice_request_picks_through_real_native_child() {
        let _guard = env_lock();
        let (driver, driver_target) = build_picker_test_driver("picker-test-driver-pick");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("browser-shaped-pick-base");
        let fixture_dir_root =
            OwnedScratchDir::create(scratch_base("browser-shaped-pick-fixture-home"));
        // Admitted only by `markdown_filter` -- sorts lexically first, so
        // an implementation that ignores filtering (or that only fixes
        // the cycle-order half of round 222's defect) would wrongly keep
        // including it whenever it is the first two "eligible" rows.
        let fixture_initial_only = fixture_dir_root.path().join("a-document.md");
        // Admitted only by `charclass_text_filter`, under a correct
        // bracket-class matcher.
        let fixture_one = fixture_dir_root.path().join("m-note-one.txt");
        let fixture_two = fixture_dir_root.path().join("z-note-two.txt");
        // Admitted by neither filter exercised in this test. Sorts
        // between `fixture_one` and `fixture_two`, so an unfiltered or
        // unswitched "take the first two eligible rows" cannot produce
        // the same pair either call below expects.
        let fixture_unmatched = fixture_dir_root.path().join("q-archive.bin");
        fs::write(&fixture_initial_only, b"d223 initial-filter-only fixture\n")
            .expect("write initial-filter-only fixture");
        fs::write(&fixture_one, b"d223 matching fixture one\n").expect("write matching fixture one");
        fs::write(&fixture_two, b"d223 matching fixture two\n").expect("write matching fixture two");
        fs::write(&fixture_unmatched, b"d223 unmatched fixture\n").expect("write unmatched fixture");

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver, Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        // Expected URIs, computed while the fixture paths are still owned
        // -- never derived after `fixture_dir_root.finish()` below.
        let first_expected_uris = vec![gio::File::for_path(&fixture_initial_only).uri().to_string()];
        let second_expected_uris = vec![
            gio::File::for_path(&fixture_one).uri().to_string(),
            gio::File::for_path(&fixture_two).uri().to_string(),
        ];

        // First real call: filters ordered `[charclass, markdown]`, so
        // `current_filter` resolves to index 0 (`charclass_text_filter`,
        // never exercised against a fixture here) and the driver's
        // forward-by-one cycle lands on index 1 (`markdown_filter`),
        // which admits exactly `fixture_initial_only`.
        let (first_response, first_results) = call_open_file(
            &client,
            real_child_filter_options(vec![charclass_text_filter(), markdown_filter()]),
        )
        .expect("first OpenFile call");

        // Second real call, same test, filters reordered to
        // `[markdown, charclass]`: `current_filter` now resolves to index
        // 0 (`markdown_filter`), and the same forward-by-one cycle lands
        // on index 1 (`charclass_text_filter`), which must admit exactly
        // `fixture_one`/`fixture_two` and refuse `fixture_initial_only` --
        // the same fixture the first call just proved that filter admits
        // under the *other* ordering. Against `75ca650`'s literal-bracket
        // matcher this filter admits nothing at all, so the real child
        // finds no eligible row, the driver never clicks Open, the
        // deadline expires, and this call reports failure instead of the
        // success asserted below: the truthful red this round proves.
        //
        // No assertion runs between the two calls, or between this call
        // and the full teardown below: an expected (red) panic here must
        // still restore `HOME`, close the private bus, restore
        // `DISPLAY`/`WAYLAND_DISPLAY`, stop Xvfb and remove every owned
        // root, not leave them live for whichever portal test runs next.
        let (second_response, second_results) = call_open_file(
            &client,
            real_child_filter_options(vec![markdown_filter(), charclass_text_filter()]),
        )
        .expect("second OpenFile call");

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be gone after both completed real picks"
        );
        finish_base(base);
        driver_target.finish();

        // Only now, with every environment variable and owned root back to
        // its exact prior state, does this test assert what either real
        // call actually returned.
        assert_eq!(
            first_response, RESPONSE_SUCCESS,
            "real pick cycled onto the Markdown filter must report success"
        );
        assert_eq!(
            uris_of(&first_results),
            first_expected_uris,
            "cycled onto Markdown, the real native child must pick exactly \
             fixture_initial_only -- never the unmatched or Text fixtures"
        );
        let first_current_filter: (String, Vec<(u32, String)>) = first_results
            .get("current_filter")
            .cloned()
            .map(|v| <(String, Vec<(u32, String)>)>::try_from(v).expect("current_filter decodes"))
            .expect("current_filter present");
        assert_eq!(
            first_current_filter,
            markdown_filter(),
            "the real dropdown's cycled-to selection must round-trip back"
        );
        let first_choices: Vec<(String, String)> = first_results
            .get("choices")
            .cloned()
            .map(|v| Vec::<(String, String)>::try_from(v).expect("choices decodes"))
            .expect("choices present");
        assert_eq!(
            first_choices,
            vec![
                ("encoding".to_string(), "latin1".to_string()),
                ("readonly".to_string(), "true".to_string()),
            ],
            "both real choice controls' cycled selections must round-trip back"
        );

        assert_eq!(
            second_response, RESPONSE_SUCCESS,
            "real pick cycled onto the strict-case Text filter must report success"
        );
        assert_eq!(
            uris_of(&second_results),
            second_expected_uris,
            "cycled onto the strict-case Text filter, the real native child must pick \
             exactly fixture_one and fixture_two -- never fixture_initial_only (proven \
             admitted under Markdown by the first call above) or the unmatched fixture"
        );
        let second_current_filter: (String, Vec<(u32, String)>) = second_results
            .get("current_filter")
            .cloned()
            .map(|v| <(String, Vec<(u32, String)>)>::try_from(v).expect("current_filter decodes"))
            .expect("current_filter present");
        assert_eq!(
            second_current_filter,
            charclass_text_filter(),
            "the real dropdown's cycled-to selection must round-trip back"
        );
        let second_choices: Vec<(String, String)> = second_results
            .get("choices")
            .cloned()
            .map(|v| Vec::<(String, String)>::try_from(v).expect("choices decodes"))
            .expect("choices present");
        assert_eq!(
            second_choices,
            vec![
                ("encoding".to_string(), "latin1".to_string()),
                ("readonly".to_string(), "true".to_string()),
            ],
            "both real choice controls' cycled selections must round-trip back"
        );
    }

    /// D-224/D-225: a `current_filter` supplied alone, with `filters`
    /// empty, must become the real admission filter through the actual
    /// native child -- never the unconstrained picker `75ca650`'s
    /// `decode_current_filter` produced by returning `None` whenever
    /// `filters` was empty -- and must round-trip back in
    /// `results.current_filter` without a fake second entry ever
    /// appearing in a (nonexistent, since `filters.len() < 2`) dropdown.
    ///
    /// D-225: the excluded fixture (`a-archive.bin`) sorts and lists
    /// *before* the admitted one (`b-note.txt`), so an unconstrained
    /// selection -- one row's worth, taken from whichever rows the test
    /// driver's own `picker_entry_can_submit` guard says match, in listing
    /// order -- would pick the excluded fixture first if that guard ever
    /// stopped actually filtering. Round 224's fixture order (matching
    /// fixture first) could not have told the two cases apart; this order
    /// can and does -- confirmed by a focused mutation run with native
    /// lone-filter admission disabled, which fails on exactly this
    /// assertion, retained in the round's gate report rather than in this
    /// file.
    #[test]
    fn dbus_openfile_lone_current_filter_without_filters_list_filters_real_native_child_and_round_trips() {
        let _guard = env_lock();
        let (driver, driver_target) = build_picker_test_driver("picker-test-driver-pick");
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("lone-current-filter-pick-base");
        let fixture_dir_root =
            OwnedScratchDir::create(scratch_base("lone-current-filter-pick-fixture-home"));
        // Excluded fixture sorts first; admitted fixture sorts second --
        // see the D-225 doc comment above for why the order matters now.
        let fixture_unmatched = fixture_dir_root.path().join("a-archive.bin");
        let fixture_matching = fixture_dir_root.path().join("b-note.txt");
        fs::write(&fixture_unmatched, b"d224 lone current_filter unmatched fixture\n")
            .expect("write unmatched fixture");
        fs::write(&fixture_matching, b"d224 lone current_filter matching fixture\n")
            .expect("write matching fixture");

        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fixture_dir_root.path());

        let chooser: Arc<dyn Chooser> =
            Arc::new(ProcessChooser::for_test(driver, Duration::from_secs(30), base.clone()));
        let server = bus.connect_server(chooser);
        let client = bus.connect_client();

        let expected_uris = vec![gio::File::for_path(&fixture_matching).uri().to_string()];
        let lone_filter = ("Text only".to_string(), vec![(0u32, "*.txt".to_string())]);

        let mut options = HashMap::new();
        options.insert("multiple".to_string(), OwnedValue::from(false));
        options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(lone_filter.clone())).expect("current_filter encodes"),
        );
        // No "filters" key at all: the caller supplied exactly one filter
        // through `current_filter` alone.

        let (response, results) = call_open_file(&client, options).expect("OpenFile call");

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        finish_private_bus(bus, server, client);
        match old_display {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        xvfb.finish();
        fixture_dir_root.finish();
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "the request root must be gone after the completed real pick"
        );
        finish_base(base);
        driver_target.finish();

        assert_eq!(
            response, RESPONSE_SUCCESS,
            "a lone current_filter with no dropdown to cycle must still filter and succeed"
        );
        assert_eq!(
            uris_of(&results),
            expected_uris,
            "the real native child must admit only the matching fixture under the lone \
             current_filter, never the unmatched one"
        );
        let round_tripped: (String, Vec<(u32, String)>) = results
            .get("current_filter")
            .cloned()
            .map(|v| <(String, Vec<(u32, String)>)>::try_from(v).expect("current_filter decodes"))
            .expect("current_filter present even though filters was empty");
        assert_eq!(
            round_tripped, lone_filter,
            "the lone supplied filter must round-trip back as the effective active filter"
        );
    }

    /// D-224: `decode_current_filter` must decode and return
    /// `current_filter` even when `filters` is empty -- `75ca650` returned
    /// `None` before decoding in that branch, which silently dropped the
    /// caller's only filter and produced an unconstrained picker.
    #[test]
    fn decode_current_filter_decodes_lone_filter_when_filters_list_is_empty() {
        let mut options = HashMap::new();
        let lone_filter = ("Text".to_string(), vec![(0u32, "*.txt".to_string())]);
        options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(lone_filter.clone())).expect("current_filter encodes"),
        );
        let decoded = decode_current_filter(&options, &[]).expect("well-formed current_filter decodes");
        assert_eq!(
            decoded,
            Some(WireFilter {
                name: lone_filter.0,
                rules: lone_filter.1
            }),
            "a lone current_filter must decode to itself, never None, when filters is empty"
        );
    }

    /// D-224: when `filters` is nonempty, `decode_current_filter` keeps
    /// resolving against it exactly as before -- an exact match wins, an
    /// absent/non-matching value falls back to the first supplied filter.
    #[test]
    fn decode_current_filter_still_resolves_against_nonempty_filters_list() {
        let filters = vec![
            WireFilter { name: "Markdown".to_string(), rules: vec![(0, "*.md".to_string())] },
            WireFilter { name: "Text".to_string(), rules: vec![(0, "*.txt".to_string())] },
        ];
        let mut matching_options = HashMap::new();
        matching_options.insert(
            "current_filter".to_string(),
            OwnedValue::try_from(Value::from(("Text".to_string(), vec![(0u32, "*.txt".to_string())])))
                .expect("current_filter encodes"),
        );
        assert_eq!(
            decode_current_filter(&matching_options, &filters).expect("decodes"),
            Some(filters[1].clone()),
            "an exact match against a nonempty filters list must select that entry"
        );

        let empty_options = HashMap::new();
        assert_eq!(
            decode_current_filter(&empty_options, &filters).expect("decodes"),
            Some(filters[0].clone()),
            "an absent current_filter against a nonempty filters list must fall back to the first"
        );
    }

    /// D-224: `write_request_file` must refuse to write when the
    /// serialized request exceeds `MAX_WIRE_JSON_BYTES`, leaving no file
    /// behind -- `75ca650` applied this same bound only in
    /// `read_result_meta`, never here, so an oversized request was written
    /// and accepted instead of refused.
    #[test]
    fn write_request_file_refuses_over_limit_write_and_leaves_no_file() {
        let dir_root = OwnedScratchDir::create(scratch_base("write-request-file-over-limit"));
        let path = dir_root.path().join("request.json");
        let oversized_name = "x".repeat(MAX_WIRE_JSON_BYTES as usize + 1);
        let request = WireRequest {
            multiple: false,
            save: false,
            current_name: String::new(),
            current_folder: None,
            current_file: None,
            filters: vec![WireFilter { name: oversized_name, rules: Vec::new() }],
            current_filter: None,
            choices: Vec::new(),
        };
        let result = write_request_file(&path, &request);
        assert!(result.is_err(), "an over-limit request must be refused, not written");
        assert!(!path.exists(), "a refused write must leave no file behind");
        dir_root.finish();
    }

    /// D-224: `write_request_file` still writes a well-formed,
    /// within-bound request normally -- the size check must refuse only
    /// what actually exceeds the bound.
    #[test]
    fn write_request_file_accepts_within_limit_request() {
        let dir_root = OwnedScratchDir::create(scratch_base("write-request-file-within-limit"));
        let path = dir_root.path().join("request.json");
        let request = WireRequest::default();
        write_request_file(&path, &request).expect("a well-formed request within bound must write");
        assert!(path.exists(), "an accepted write must leave the file behind");
        dir_root.finish();
    }

    /// D-224: `decode_choices` must refuse, before the chooser is ever
    /// called, every one of the installed shape's forbidden empty fields
    /// and invalid nonempty initials -- `75ca650` had no such validation at
    /// all.
    #[test]
    fn decode_choices_refuses_every_forbidden_empty_field_and_invalid_initial() {
        let empty_choice_id = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "".to_string(),
                "Label".to_string(),
                Vec::<(String, String)>::new(),
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(decode_choices(&empty_choice_id).is_err(), "an empty choice id must be refused");

        let empty_choice_label = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "".to_string(),
                Vec::<(String, String)>::new(),
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(decode_choices(&empty_choice_label).is_err(), "an empty choice label must be refused");

        let empty_option_id = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                vec![("".to_string(), "Option".to_string())],
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(decode_choices(&empty_option_id).is_err(), "an empty option id must be refused");

        let empty_option_label = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                vec![("opt".to_string(), "".to_string())],
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(decode_choices(&empty_option_label).is_err(), "an empty option label must be refused");

        let bad_enum_initial = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                vec![("opt".to_string(), "Option".to_string())],
                "not-an-option-id".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(
            decode_choices(&bad_enum_initial).is_err(),
            "an enumerated initial naming no real option id must be refused"
        );

        let bad_bool_initial = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                Vec::<(String, String)>::new(),
                "maybe".to_string(),
            )]))
            .expect("encodes"),
        )]);
        assert!(
            decode_choices(&bad_bool_initial).is_err(),
            "a boolean initial other than \"\"/\"true\"/\"false\" must be refused"
        );
    }

    /// D-225: every malformed-choices case above must refuse *through
    /// `decide`*, before the chooser closure is ever invoked -- the
    /// no-chooser-call invariant lives in `decide`'s early `?`-style
    /// returns, not in `decode_choices` alone, so this calls the real
    /// `decide` with a closure that records its own invocation count and
    /// asserts both the refusal response and zero calls, for each case.
    #[test]
    fn decide_refuses_every_malformed_choices_case_without_ever_calling_the_chooser() {
        let cases: Vec<(&str, HashMap<String, OwnedValue>)> = vec![
            (
                "empty choice id",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "".to_string(),
                        "Label".to_string(),
                        Vec::<(String, String)>::new(),
                        "".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
            (
                "empty choice label",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "id".to_string(),
                        "".to_string(),
                        Vec::<(String, String)>::new(),
                        "".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
            (
                "empty option id",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "id".to_string(),
                        "Label".to_string(),
                        vec![("".to_string(), "Option".to_string())],
                        "".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
            (
                "empty option label",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "id".to_string(),
                        "Label".to_string(),
                        vec![("opt".to_string(), "".to_string())],
                        "".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
            (
                "bad enum initial",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "id".to_string(),
                        "Label".to_string(),
                        vec![("opt".to_string(), "Option".to_string())],
                        "not-an-option-id".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
            (
                "bad bool initial",
                HashMap::from([(
                    "choices".to_string(),
                    OwnedValue::try_from(Value::from(vec![(
                        "id".to_string(),
                        "Label".to_string(),
                        Vec::<(String, String)>::new(),
                        "maybe".to_string(),
                    )]))
                    .expect("encodes"),
                )]),
            ),
        ];

        for (label, options) in cases {
            let calls = std::cell::Cell::new(0u32);
            let (response, results) = decide(
                &options,
                |_request| {
                    calls.set(calls.get() + 1);
                    ChooserOutcome::Cancelled.into()
                },
                GuardOverride::None,
            );
            assert_eq!(calls.get(), 0, "{label}: the chooser must never be called for a malformed choices request");
            assert_eq!(response, RESPONSE_OTHER, "{label}: decide must refuse, not cancel or succeed");
            assert!(results.is_empty(), "{label}: a refusal carries no results");
        }
    }

    /// D-224: an empty `initial` resolves to the portal's own choice --
    /// the first option for an enumerated choice, `"false"` for a
    /// boolean one -- rather than being refused.
    #[test]
    fn decode_choices_resolves_empty_initial_to_portal_chosen_value() {
        let enumerated = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                vec![
                    ("first".to_string(), "First".to_string()),
                    ("second".to_string(), "Second".to_string()),
                ],
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        let decoded = decode_choices(&enumerated).expect("well-formed choice decodes");
        assert_eq!(decoded[0].initial, "first", "empty enumerated initial must resolve to the first option");

        let boolean = HashMap::from([(
            "choices".to_string(),
            OwnedValue::try_from(Value::from(vec![(
                "id".to_string(),
                "Label".to_string(),
                Vec::<(String, String)>::new(),
                "".to_string(),
            )]))
            .expect("encodes"),
        )]);
        let decoded = decode_choices(&boolean).expect("well-formed choice decodes");
        assert_eq!(decoded[0].initial, "false", "empty boolean initial must resolve to \"false\"");
    }

    // --- Round 215: cold D-Bus activation of the real backend through the
    // real `/usr/libexec/xdg-desktop-portal` frontend, driven by on-disk
    // `.portal`/`.service` integration files materialized under a fully
    // private `HOME`/XDG environment, never the owner's real one. ---

    const TEST_APP_INFO_KIND_HOST: &str = "host";
    const TEST_HOST_APP_ID: &str = "org.wayfinder.round215.test";
    const FRONTEND_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
    const FRONTEND_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
    const FRONTEND_INTERFACE: &str = "org.freedesktop.portal.FileChooser";
    const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";

    /// Owns one fresh, uniquely named root below `~/Projects/Scratch/`
    /// holding a private `HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and
    /// `XDG_RUNTIME_DIR` (each mode `0700`), and materializes the
    /// `.portal`/`.service` integration files beneath it. Every relevant
    /// environment variable is saved before being overridden and restored
    /// on `finish()`/`Drop`; `Drop` alone never panics, so it is safe
    /// during unwind.
    struct PrivatePortalEnv {
        // D-207: owns the one exact fresh activation root -- whatever the
        // caller already placed in it (such as a Cargo `target/` sibling
        // from `build_portal_activation_artifacts`) plus this env's own
        // `env/` child -- so Cargo output and the private HOME/XDG/
        // runtime/descriptor/service fixture live beneath a single
        // unwind-safe owner instead of two separately owned roots.
        owner: Option<OwnedScratchDir>,
        env_root: PathBuf,
        data_home: PathBuf,
        desktop_id: String,
        saved: Vec<(&'static str, Option<String>)>,
        restored: Cell<bool>,
    }

    impl PrivatePortalEnv {
        const VARS: &'static [&'static str] = &[
            "HOME",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "XDG_RUNTIME_DIR",
            "XDG_DESKTOP_PORTAL_DIR",
            "XDG_CURRENT_DESKTOP",
            "XDG_DESKTOP_PORTAL_TEST_APP_INFO_KIND",
            "XDG_DESKTOP_PORTAL_TEST_HOST_APPID",
            "DISPLAY",
            "WAYLAND_DISPLAY",
        ];

        /// A standalone private env with its own exact fresh root. D-208:
        /// the owner exists, and its unwind-safe `Drop` fallback is live,
        /// before the root itself is created, so no panic interval in
        /// between can leave either the files or the process-wide vars
        /// unowned.
        fn start() -> Self {
            let home_var = std::env::var("HOME").expect("HOME must be set");
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let root = PathBuf::from(home_var).join("Projects/Scratch").join(format!(
                "d215-portal-env-{}-{}-{}",
                std::process::id(),
                stamp,
                next_unique_id()
            ));
            let owner = OwnedScratchDir::create(root);
            let env_root = owner.path().to_path_buf();
            Self::build(owner, env_root)
        }

        /// D-207: nests this env's `env/` fixture tree inside `owner`'s
        /// already-created exact root, transferring ownership of that
        /// whole root -- Cargo `target/` sibling included -- into this
        /// `PrivatePortalEnv`. `owner` already exists before this method
        /// writes a single file or touches a single environment variable.
        fn start_under(owner: OwnedScratchDir) -> Self {
            let env_root = owner.path().join("env");
            Self::build(owner, env_root)
        }

        fn build(owner: OwnedScratchDir, env_root: PathBuf) -> Self {
            let home = env_root.join("home");
            let data_home = home.join(".local/share");
            let config_home = home.join(".config");
            let runtime_dir = env_root.join("run");
            let portals_dir = data_home.join("xdg-desktop-portal/portals");
            let services_dir = data_home.join("dbus-1/services");
            for dir in [&env_root, &home, &data_home, &config_home, &runtime_dir, &portals_dir, &services_dir] {
                fs::create_dir_all(dir).expect("create private portal-env directory");
            }
            for dir in [&home, &data_home, &config_home, &runtime_dir] {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                    .expect("restrict private portal-env directory");
            }

            let desktop_id = format!("WayfinderTest{}", next_unique_id());
            let saved: Vec<(&'static str, Option<String>)> = Self::VARS
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();

            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CONFIG_HOME", &config_home);
            std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
            std::env::set_var("XDG_DESKTOP_PORTAL_DIR", &portals_dir);
            std::env::set_var("XDG_CURRENT_DESKTOP", &desktop_id);
            std::env::set_var("XDG_DESKTOP_PORTAL_TEST_APP_INFO_KIND", TEST_APP_INFO_KIND_HOST);
            std::env::set_var("XDG_DESKTOP_PORTAL_TEST_HOST_APPID", TEST_HOST_APP_ID);

            Self {
                owner: Some(owner),
                env_root,
                data_home,
                desktop_id,
                saved,
                restored: Cell::new(false),
            }
        }

        fn root(&self) -> &Path {
            &self.env_root
        }

        /// The one shared round root itself (the parent of this env's own
        /// `env/` child and of the build's `target/` sibling), one path
        /// segment shorter than `root()`. D-207: `AF_UNIX` socket paths
        /// are capped at `sun_path`'s ~108 bytes, and nesting the
        /// activation-bus socket under the deeper `root()` (`env/`) on
        /// top of the merged one-root's longer label pushed real runs
        /// over that cap, so the private activation bus binds here
        /// instead, directly under the shorter shared root.
        fn round_root(&self) -> &Path {
            self.owner.as_ref().expect("owner present until finish").path()
        }

        fn runtime_dir(&self) -> PathBuf {
            self.env_root.join("run")
        }

        fn portals_dir(&self) -> PathBuf {
            self.data_home.join("xdg-desktop-portal/portals")
        }

        fn services_dir(&self) -> PathBuf {
            self.data_home.join("dbus-1/services")
        }

        fn desktop_id(&self) -> &str {
            &self.desktop_id
        }

        /// Points `DISPLAY` at the private Xvfb and clears `WAYLAND_DISPLAY`,
        /// process-wide, so the exact ambient environment any later-spawned
        /// process inherits -- most importantly the activation
        /// `dbus-daemon`, which hands this same environment to whatever it
        /// cold-activates -- can only ever reach the private display, never
        /// Zito's real one. Must be called after the private Xvfb is up and
        /// before the private activation bus (or anything else) starts; the
        /// prior real value was already captured by `start()`'s own save,
        /// so `finish()`/`Drop` restore it like every other private var.
        fn activate_private_display(&self, display: &str) {
            std::env::set_var("DISPLAY", display);
            std::env::remove_var("WAYLAND_DISPLAY");
        }

        /// Writes `wayfinder.portal` naming only the `FileChooser`
        /// interface and this env's own unique `UseIn` value, so no other
        /// desktop session on the machine could ever select it.
        fn write_portal_descriptor(&self) {
            let contents = format!(
                "[portal]\nDBusName={}\nInterfaces=org.freedesktop.impl.portal.FileChooser;\nUseIn={}\n",
                BUS_NAME, self.desktop_id
            );
            fs::write(self.portals_dir().join("wayfinder.portal"), contents)
                .expect("write the private .portal descriptor");
        }

        /// Writes `wayfinder.portal` with the given field overridden with
        /// `value`, for a named refusal/bypass pair.
        fn write_portal_descriptor_with(&self, field: &str, value: &str) {
            let contents = format!(
                "[portal]\n{}\n",
                [
                    ("DBusName", BUS_NAME.to_string()),
                    (
                        "Interfaces",
                        "org.freedesktop.impl.portal.FileChooser;".to_string()
                    ),
                    ("UseIn", self.desktop_id.clone()),
                ]
                .into_iter()
                .map(|(key, default)| {
                    let v = if key == field { value.to_string() } else { default };
                    format!("{key}={v}")
                })
                .collect::<Vec<_>>()
                .join("\n")
            );
            fs::write(self.portals_dir().join("wayfinder.portal"), contents)
                .expect("write the private .portal descriptor");
        }

        /// Writes the `.service` activation file whose `Exec` resolves
        /// directly to the round-owned `wayfinder-portal` test artifact --
        /// no shell, no systemd user unit.
        fn write_service_file(&self, exec_path: &Path) {
            self.write_service_file_with("Name", BUS_NAME, exec_path);
        }

        fn write_service_file_with(&self, field: &str, value: &str, exec_path: &Path) {
            let contents = format!(
                "[D-BUS Service]\n{}\n",
                [
                    ("Name", BUS_NAME.to_string()),
                    ("Exec", exec_path.display().to_string()),
                ]
                .into_iter()
                .map(|(key, default)| {
                    let v = if key == field { value.to_string() } else { default };
                    format!("{key}={v}")
                })
                .collect::<Vec<_>>()
                .join("\n")
            );
            fs::write(
                self.services_dir()
                    .join("org.freedesktop.impl.portal.desktop.wayfinder.service"),
                contents,
            )
            .expect("write the private .service activation file");
        }

        /// Explicitly, observably removes the one exact owned root
        /// (Cargo `target/` sibling included, when there is one) and
        /// restores every saved environment variable. Failure to remove
        /// fails the test; `Drop`'s fallback is skipped afterward.
        fn finish(mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
            self.restored.set(true);
            self.owner.take().expect("owner present until finish").finish();
        }
    }

    impl Drop for PrivatePortalEnv {
        fn drop(&mut self) {
            if self.restored.get() {
                return;
            }
            for (name, value) in &self.saved {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
            // `self.owner`'s own `Drop` runs right after this function
            // returns (field drop order), performing the bounded,
            // non-panicking unwind removal of the one exact root.
        }
    }

    /// Builds `wayfinder-portal` and `wayfinder-native` together into one
    /// round-owned target dir below `~/Projects/Scratch/`, with `feature`
    /// (one of the `picker-test-driver-{pick,cancel}` sub-features) baked
    /// into the `wayfinder-native` sibling. `ProcessChooser::production()`
    /// resolves `wayfinder-native` by looking next to its own exe, so
    /// placing both binaries in the same directory is what lets the
    /// unmodified production chooser find the feature-built driver.
    ///
    /// D-206: the round root is created and wrapped in `OwnedScratchDir`
    /// -- establishing its unwind-safe owner -- *before* `cargo` ever
    /// runs, and the actual Cargo target lives as a `target` subdir
    /// inside that already-owned root. There is no setup, spawn,
    /// status-check or assertion interval in which the round root (or
    /// anything Cargo creates under it) exists without an owner: a panic
    /// from any `assert!`/`.expect(...)` below is caught by `Drop`.
    fn build_portal_activation_artifacts(feature: &str) -> (PathBuf, PrivatePortalEnv) {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        // D-207: the merged one-root now also carries the private
        // activation bus's `AF_UNIX` socket (see `round_root()`), whose
        // `sun_path` is capped at ~108 bytes, so the round-root label
        // stays short here rather than spelling out the full feature name
        // the way other, non-socket-bearing `scratch_base()` callers do.
        let short_feature = feature.replace("picker-test-driver-", "ptd-");
        let round_root = scratch_base(&format!("pat-{short_feature}"));
        let round_root = OwnedScratchDir::create(round_root);
        let target_dir = round_root.path().join("target");
        let status = Command::new("cargo")
            .arg("build")
            .arg("--quiet")
            .arg("--bin")
            .arg("wayfinder-portal")
            .arg("--bin")
            .arg("wayfinder-native")
            .arg("--features")
            .arg(feature)
            .arg("--target-dir")
            .arg(&target_dir)
            .current_dir(manifest_dir)
            .status()
            .expect("run cargo build for the round-owned portal/native artifacts");
        assert!(status.success(), "building the {feature} portal/native artifacts failed");
        let portal_bin = target_dir.join("debug").join("wayfinder-portal");
        assert!(portal_bin.exists(), "expected portal binary at {}", portal_bin.display());
        let native_bin = target_dir.join("debug").join("wayfinder-native");
        assert!(native_bin.exists(), "expected native binary at {}", native_bin.display());
        let env = PrivatePortalEnv::start_under(round_root);
        (portal_bin, env)
    }

    /// A fresh, unique D-206 token for one oracle run, and the exact
    /// top-level `~/Projects/Scratch` prefix built from it. Exact-prefix
    /// only: never a broad glob, so a pre-existing entry (e.g. the
    /// rejected-215 `d205-portal-activation-target-...` directory) can
    /// never match.
    fn d206_fresh_token() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        format!("d206-owner-oracle-{}-{}-{}", std::process::id(), stamp, next_unique_id())
    }

    /// Counts only top-level `~/Projects/Scratch` entries whose file name
    /// is exactly this fresh token's directory name -- never a glob over
    /// `d206-*`, so it cannot see any other run's or round's entries.
    fn d206_prefix_count(token: &str) -> usize {
        let home = std::env::var("HOME").expect("HOME must be set");
        let scratch = PathBuf::from(home).join("Projects/Scratch");
        fs::read_dir(&scratch)
            .expect("read Scratch root")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy() == token)
            .count()
    }

    /// Builds the same nontrivial, Cargo-target-shaped evidence subtree
    /// every D-206 oracle below creates: a `target/debug/` directory
    /// holding a stand-in binary and a build fingerprint file, proving
    /// the oracle is exercised against real build-shaped output and not
    /// an empty directory.
    fn d206_populate_target_shaped_subtree(root: &Path) {
        let debug_dir = root.join("target").join("debug");
        fs::create_dir_all(&debug_dir).expect("create target-shaped evidence subtree");
        fs::write(debug_dir.join("wayfinder-portal"), b"d206 evidence stand-in binary\n")
            .expect("write evidence stand-in binary");
        fs::write(debug_dir.join(".fingerprint-marker"), b"d206 evidence build fingerprint\n")
            .expect("write evidence build fingerprint");
    }

    /// Round 216 (D-206) item 3: the named exact-prefix survival oracle.
    /// A fresh, run-unique token names a round root that is created,
    /// wrapped in `OwnedScratchDir` (the same owner `build_portal_activation_artifacts`
    /// now establishes before Cargo ever runs), populated with a
    /// nontrivial Cargo-target-shaped subtree, and explicitly finished.
    /// The oracle then enumerates Scratch by that exact token and
    /// requires zero matches.
    #[test]
    fn d206_owner_exact_prefix_leaves_zero_entries_after_finish() {
        let _guard = env_lock();
        let token = d206_fresh_token();
        let home = std::env::var("HOME").expect("HOME must be set");
        let root_path = PathBuf::from(&home).join("Projects/Scratch").join(&token);
        let owner = OwnedScratchDir::create(root_path);
        d206_populate_target_shaped_subtree(owner.path());

        owner.finish();

        assert_eq!(
            d206_prefix_count(&token),
            0,
            "the owner's finish() must leave zero entries for its exact fresh prefix"
        );
    }

    /// Round 219 item 1: the paired deletion-bypass proof. Builds the
    /// identical created tree and evidence as the oracle above, under its
    /// own fresh token, but observes the live owner's survivor through
    /// `peek_while_live` -- never `mem::forget` -- to prove the exact same
    /// oracle actually detects a real survivor rather than passing
    /// unconditionally, while the owner's unwind-safe `Drop` fallback
    /// stays armed throughout. The proof then calls checked `finish()`,
    /// so the proof itself leaves nothing behind, and there is never an
    /// unowned interval. Mechanically equivalent to the D-208 bypass
    /// proof below (same owner, same shapes), kept because no test
    /// deletion is authorized.
    #[test]
    fn d206_owner_exact_prefix_bypass_proves_oracle_detects_survivor() {
        let _guard = env_lock();
        let token = d206_fresh_token();
        let home = std::env::var("HOME").expect("HOME must be set");
        let root_path = PathBuf::from(&home).join("Projects/Scratch").join(&token);
        let owner = OwnedScratchDir::create(root_path);
        d206_populate_target_shaped_subtree(owner.path());

        // The owner is still live here -- no `mem::forget`, no early
        // `finish()` -- so `Drop` would still reap the exact root if this
        // peek itself panicked or timed out.
        owner.peek_while_live(|| {
            assert_eq!(
                d206_prefix_count(&token),
                1,
                "with the owner still live and unremoved, the oracle must detect the survivor"
            );
        });

        owner.finish();
        assert_eq!(
            d206_prefix_count(&token),
            0,
            "the bypass proof's own checked finish() must leave nothing behind"
        );
    }

    /// Round 216 (D-206) item 4: forced-unwind coverage. An already-created
    /// owner -- holding a nontrivial Cargo-target-shaped subtree, the same
    /// shape `build_portal_activation_artifacts` leaves behind -- is moved
    /// into a panicking closure. Proves the unwind-safe `Drop` fallback
    /// removes the exact fresh root even though the panic happens after
    /// the Cargo-shaped tree already exists, mirroring a real assertion
    /// failure partway through `build_portal_activation_artifacts`.
    #[test]
    fn d206_owner_drop_recovers_populated_target_root_on_panic() {
        let _guard = env_lock();
        let token = d206_fresh_token();
        let home = std::env::var("HOME").expect("HOME must be set");
        let root_path = PathBuf::from(&home).join("Projects/Scratch").join(&token);
        let owner = OwnedScratchDir::create(root_path);
        d206_populate_target_shaped_subtree(owner.path());

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owner = owner;
            panic!("deliberate panic after the Cargo-target-shaped subtree already exists, before finish()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        assert_eq!(
            d206_prefix_count(&token),
            0,
            "Drop must reap the exact fresh root, populated subtree included, on unwind"
        );
    }

    /// Round 217 (D-207) proof 1: drives the real
    /// `build_portal_activation_artifacts`/`PrivatePortalEnv::start_under`
    /// path (not a synthetic stand-in) and proves the one real round root
    /// is the single shared owner of both children. The returned portal
    /// binary must sit beneath `<one-root>/target/`, and the returned
    /// env's own root must sit beneath `<one-root>/env/` -- the exact
    /// nesting the D-207 blueprint requires. After a checked `finish()`,
    /// the oracle enumerates Scratch by that one root's own exact
    /// top-level name and requires zero.
    #[test]
    fn d207_real_activation_setup_shares_one_root_and_leaves_zero_entries() {
        let _guard = env_lock();
        let (portal_bin, env) = build_portal_activation_artifacts("picker-test-driver-pick");

        let target_dir = portal_bin.parent().expect("portal bin has a parent dir");
        let one_root = target_dir
            .parent()
            .expect("target/debug has a parent")
            .parent()
            .expect("target has a parent: the one shared round root");
        let token = one_root
            .file_name()
            .expect("one-root has a file name")
            .to_string_lossy()
            .into_owned();

        assert!(
            portal_bin.starts_with(one_root.join("target")),
            "the real portal binary must sit beneath <one-root>/target/"
        );
        assert!(
            env.root().starts_with(one_root.join("env")),
            "the real env's own root must sit beneath <one-root>/env/"
        );

        env.finish();

        assert_eq!(
            d206_prefix_count(&token),
            0,
            "finishing the one shared owner must leave zero entries for the real round root's exact name"
        );
    }

    /// Builds the same `env/home/...` HOME/XDG-shaped evidence the real
    /// `PrivatePortalEnv::build()` writes beneath `<one-root>/env/`, but
    /// without mutating any real process-wide environment variable --
    /// these two proofs are about the one-root *directory* ownership, not
    /// about variable restoration (already covered by proof 1 and the
    /// unchanged 215 proofs), so they must not leave real `HOME`/`XDG_*`
    /// pointed at a root they are about to remove.
    fn d207_populate_env_shaped_subtree(root: &Path) {
        let home_dir = root.join("env").join("home");
        let data_home = home_dir.join(".local/share");
        fs::create_dir_all(&data_home).expect("create env-shaped HOME/XDG evidence subtree");
        fs::write(data_home.join("marker"), b"d207 evidence HOME/XDG fixture\n")
            .expect("write env-shaped evidence marker");
    }

    /// A fresh, unique D-208 token for one bypass run, in its own
    /// namespace -- never `d206-` or `d207-`, so this proof never borrows
    /// another round's prefix.
    fn d208_fresh_token() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        format!("d208-one-root-bypass-{}-{}-{}", std::process::id(), stamp, next_unique_id())
    }

    /// Round 218 (D-208) proof 2: the paired owner-removal-bypass proof,
    /// built on the same one-root shape (a `target/` and an `env/` child
    /// under one fresh owner) as proof 1, rewritten so there is never an
    /// unowned interval. The owner stays live throughout: the exact-prefix
    /// oracle observes the survivor through `peek_while_live` while the
    /// owner's unwind-safe `Drop` fallback is still armed, then checked
    /// `finish()` removes it normally. No `mem::forget` anywhere, and any
    /// panic, timeout or other unwind during the peek would still be
    /// reaped by `Drop`.
    #[test]
    fn d208_one_root_bypass_proves_oracle_detects_survivor_without_unowned_interval() {
        let _guard = env_lock();
        let token = d208_fresh_token();
        let home = std::env::var("HOME").expect("HOME must be set");
        let root_path = PathBuf::from(&home).join("Projects/Scratch").join(&token);
        let owner = OwnedScratchDir::create(root_path);
        d206_populate_target_shaped_subtree(owner.path());
        d207_populate_env_shaped_subtree(owner.path());

        assert!(owner.path().join("env").join("home").is_dir(), "env child must nest under <one-root>/env/");

        // The owner is still live here -- no `mem::forget`, no early
        // `finish()` -- so `Drop` would still reap the whole one-root if
        // this peek itself panicked or timed out.
        owner.peek_while_live(|| {
            assert_eq!(
                d206_prefix_count(&token),
                1,
                "with the owner still live and unremoved, the oracle must detect the one-root survivor"
            );
        });

        owner.finish();
        assert_eq!(d206_prefix_count(&token), 0, "the proof's own checked finish() must leave nothing behind");
    }

    /// Round 217 (D-207) proof 3: forced-unwind coverage taken only after
    /// BOTH the Cargo-target-shaped output and the HOME/XDG-shaped private
    /// env fixtures already exist under the one shared root, proving the
    /// whole one-root -- target and env children both -- is reaped by
    /// `Drop` on unwind, not just whichever child existed first.
    #[test]
    fn d207_one_root_drop_recovers_whole_root_on_panic_after_both_children_exist() {
        let _guard = env_lock();
        let token = d206_fresh_token();
        let home = std::env::var("HOME").expect("HOME must be set");
        let root_path = PathBuf::from(&home).join("Projects/Scratch").join(&token);
        let owner = OwnedScratchDir::create(root_path);
        d206_populate_target_shaped_subtree(owner.path());
        d207_populate_env_shaped_subtree(owner.path());
        // Both children -- `target/` and `env/` -- already exist by this
        // point, mirroring the real one-root shape after a build and a
        // `PrivatePortalEnv::start_under` call.
        assert!(owner.path().join("env").join("home").is_dir(), "the env child must already exist");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owner = owner;
            panic!("deliberate panic after both the target- and env-shaped children already exist");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        assert_eq!(
            d206_prefix_count(&token),
            0,
            "Drop must reap the whole one-root, target and env children both, on unwind"
        );
    }

    /// A private, round-owned session bus for activation: unlike
    /// `PrivateBus`, it does not touch `TMPDIR`/`HOME` itself (the caller's
    /// `PrivatePortalEnv` already owns that); it only spawns the daemon
    /// under the already-private environment (so the daemon's own
    /// `XDG_DATA_HOME` is the private one, letting its standard session
    /// service-activation directories resolve to our private
    /// `.service` file) and owns reaping it.
    struct PrivateActivationBus {
        address: String,
        daemon: Child,
        reaped: Cell<bool>,
    }

    impl PrivateActivationBus {
        fn start(root: &Path) -> Self {
            let socket_path = root.join("activation-bus.sock");
            let mut daemon = Command::new("dbus-daemon")
                .arg("--session")
                .arg(format!("--address=unix:path={}", socket_path.display()))
                .arg("--nofork")
                .arg("--print-address")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn a private activation dbus-daemon");
            let mut reader = BufReader::new(daemon.stdout.take().expect("daemon stdout piped"));
            let mut address = String::new();
            reader
                .read_line(&mut address)
                .expect("read private activation bus address");
            let address = address.trim().to_string();
            assert!(!address.is_empty(), "private activation dbus-daemon printed no address");
            Self {
                address,
                daemon,
                reaped: Cell::new(false),
            }
        }

        fn address(&self) -> &str {
            &self.address
        }

        fn connect(&self) -> Connection {
            Builder::address(self.address.as_str())
                .expect("valid private activation bus address")
                .build()
                .expect("build the activation-bus client connection")
        }

        fn finish(mut self) {
            let pid = self.daemon.id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match self.daemon.try_wait().expect("poll the private activation daemon") {
                    Some(_) => break,
                    None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                    None => {
                        let _ = self.daemon.kill();
                        self.daemon.wait().expect("reap private activation daemon after SIGKILL");
                        break;
                    }
                }
            }
            self.reaped.set(true);
            assert!(
                !PathBuf::from(format!("/proc/{pid}")).exists(),
                "private activation dbus-daemon must be gone after finish()"
            );
        }
    }

    impl Drop for PrivateActivationBus {
        fn drop(&mut self) {
            if self.reaped.get() {
                return;
            }
            let pid = self.daemon.id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            for _ in 0..50 {
                if matches!(self.daemon.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    fn name_has_owner(conn: &Connection, name: &str) -> bool {
        let proxy = zbus::blocking::fdo::DBusProxy::new(conn).expect("build the private bus's DBus proxy");
        proxy
            .name_has_owner(zbus::names::BusName::try_from(name).expect("valid bus name"))
            .unwrap_or(false)
    }

    fn wait_for_name_owner(conn: &Connection, name: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !name_has_owner(conn, name) {
            assert!(Instant::now() < deadline, "{name} never acquired an owner in time");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn connection_unix_process_id(conn: &Connection, name: &str) -> Option<u32> {
        let proxy = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
        proxy
            .get_connection_unix_process_id(zbus::names::BusName::try_from(name).ok()?)
            .ok()
    }

    /// Spawns the real `/usr/libexec/xdg-desktop-portal` frontend inside
    /// the given private bus/display, with `XDG_DESKTOP_PORTAL_DIR`
    /// already pointed at the private portals dir (via `PrivatePortalEnv`)
    /// so no installed backend can be selected for this process.
    fn spawn_portal_frontend(display: &str, bus_address: &str) -> Child {
        Command::new("/usr/libexec/xdg-desktop-portal")
            .env("DISPLAY", display)
            .env_remove("WAYLAND_DISPLAY")
            .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the real xdg-desktop-portal frontend")
    }

    fn terminate_and_reap(child: &mut Child) {
        let pid = child.id() as i32;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Err(_) => break,
            }
        }
    }

    fn terminate_and_reap_pid(pid: u32) {
        let pid = pid as i32;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while PathBuf::from(format!("/proc/{pid}")).exists() {
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Calls the real frontend's public `OpenFile`, then waits on the
    /// returned request object's own `Response` signal -- the real public
    /// two-step protocol, not the single-call backend shortcut the other
    /// tests in this file use directly.
    fn call_frontend_open_file(conn: &Connection) -> (u32, HashMap<String, OwnedValue>) {
        let proxy = zbus::blocking::Proxy::new(
            conn,
            FRONTEND_BUS_NAME,
            FRONTEND_OBJECT_PATH,
            FRONTEND_INTERFACE,
        )
        .expect("build the frontend FileChooser proxy");
        let options: HashMap<String, OwnedValue> = HashMap::new();
        let handle: zbus::zvariant::OwnedObjectPath = proxy
            .call("OpenFile", &("", "d215 round-215 cold activation", options))
            .expect("call the real frontend's OpenFile");

        let request = zbus::blocking::Proxy::new(conn, FRONTEND_BUS_NAME, handle.as_str(), REQUEST_INTERFACE)
            .expect("build the Request proxy for the returned handle");
        let mut signals = request.receive_signal("Response").expect("subscribe to Request::Response");
        let message = signals
            .next()
            .expect("the real frontend must emit exactly one Response signal");
        message
            .body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>()
            .expect("deserialize the real Response signal body")
    }

    #[test]
    fn cold_activation_real_pick_through_real_frontend() {
        let _guard = env_lock();
        let (portal_bin, env) = build_portal_activation_artifacts("picker-test-driver-pick");

        let xvfb = PrivateXvfb::start();
        env.activate_private_display(&xvfb.display);

        let fixture_file = env.root().join("home").join("d215-cold-pick-fixture.txt");
        fs::write(&fixture_file, b"d215 round 215 cold-activation fixture\n").expect("write fixture file");
        env.write_portal_descriptor();
        env.write_service_file(&portal_bin);

        let bus = PrivateActivationBus::start(env.round_root());
        let client = bus.connect();

        assert!(
            !name_has_owner(&client, BUS_NAME),
            "the backend bus name must have no owner before the first request"
        );

        let mut frontend = spawn_portal_frontend(&xvfb.display, bus.address());
        wait_for_name_owner(&client, FRONTEND_BUS_NAME, Duration::from_secs(10));

        let (response, results) = call_frontend_open_file(&client);
        let backend_pid = connection_unix_process_id(&client, BUS_NAME);

        let expected_uri = gio::File::for_path(&fixture_file).uri().to_string();
        assert_eq!(response, RESPONSE_SUCCESS, "cold-activated real pick must report success");
        assert_eq!(
            uris_of(&results),
            vec![expected_uri],
            "the exported URI must be the real driver's exact selected fixture, via real activation"
        );

        drop(client);
        terminate_and_reap(&mut frontend);
        if let Some(pid) = backend_pid {
            terminate_and_reap_pid(pid);
        }
        bus.finish();
        xvfb.finish();
        env.finish();
    }

    #[test]
    fn cold_activation_real_cancel_through_real_frontend() {
        let _guard = env_lock();
        let (portal_bin, env) = build_portal_activation_artifacts("picker-test-driver-cancel");

        let xvfb = PrivateXvfb::start();
        env.activate_private_display(&xvfb.display);

        fs::write(
            env.root().join("home").join("d215-cold-cancel-fixture.txt"),
            b"d215 round 215 cold-activation fixture\n",
        )
        .expect("write fixture file");
        env.write_portal_descriptor();
        env.write_service_file(&portal_bin);

        let bus = PrivateActivationBus::start(env.round_root());
        let client = bus.connect();

        assert!(
            !name_has_owner(&client, BUS_NAME),
            "the backend bus name must have no owner before the first request"
        );

        let mut frontend = spawn_portal_frontend(&xvfb.display, bus.address());
        wait_for_name_owner(&client, FRONTEND_BUS_NAME, Duration::from_secs(10));

        let (response, results) = call_frontend_open_file(&client);
        let backend_pid = connection_unix_process_id(&client, BUS_NAME);

        assert_eq!(response, RESPONSE_CANCELLED, "cold-activated real Cancel must report cancelled");
        assert!(uris_of(&results).is_empty(), "a real Cancel must carry no uri");

        drop(client);
        terminate_and_reap(&mut frontend);
        if let Some(pid) = backend_pid {
            terminate_and_reap_pid(pid);
        }
        bus.finish();
        xvfb.finish();
        env.finish();
    }

    // --- Round 215 item 6: named same-evidence refusal/bypass pairs at
    // every new routing/activation decision. Each refusal test changes
    // only the one named field/file and proves the public frontend
    // request refuses without a false pick and without activating the
    // real backend; its paired bypass restores only that one decision,
    // holding bus/binaries/request/fixture fixed, and proves routing
    // succeeds. ---

    #[derive(Debug)]
    enum FrontendAttempt {
        Responded(u32, HashMap<String, OwnedValue>),
        CallFailed(String),
        TimedOut,
    }

    /// Runs the real public `OpenFile` round trip entirely inside its own
    /// thread, owning its own bus connection, and bounds the whole attempt
    /// (including the initial call) by `timeout` so a refusal that never
    /// replies can never hang the test. Killing the frontend after this
    /// returns unblocks any leftover thread still waiting on the signal.
    fn attempt_frontend_open_file_bounded(bus_address: String, timeout: Duration) -> FrontendAttempt {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let outcome = (|| -> FrontendAttempt {
                let conn = match Builder::address(bus_address.as_str()).and_then(|b| b.build()) {
                    Ok(conn) => conn,
                    Err(e) => return FrontendAttempt::CallFailed(e.to_string()),
                };
                let proxy = match zbus::blocking::Proxy::new(
                    &conn,
                    FRONTEND_BUS_NAME,
                    FRONTEND_OBJECT_PATH,
                    FRONTEND_INTERFACE,
                ) {
                    Ok(p) => p,
                    Err(e) => return FrontendAttempt::CallFailed(e.to_string()),
                };
                let options: HashMap<String, OwnedValue> = HashMap::new();
                let handle: Result<zbus::zvariant::OwnedObjectPath, zbus::Error> =
                    proxy.call("OpenFile", &("", "d215 routing probe", options));
                let handle = match handle {
                    Ok(h) => h,
                    Err(e) => return FrontendAttempt::CallFailed(e.to_string()),
                };
                let request =
                    match zbus::blocking::Proxy::new(&conn, FRONTEND_BUS_NAME, handle.as_str(), REQUEST_INTERFACE) {
                        Ok(p) => p,
                        Err(e) => return FrontendAttempt::CallFailed(e.to_string()),
                    };
                let mut signals = match request.receive_signal("Response") {
                    Ok(s) => s,
                    Err(e) => return FrontendAttempt::CallFailed(e.to_string()),
                };
                match signals.next() {
                    Some(message) => match message.body().deserialize::<(u32, HashMap<String, OwnedValue>)>() {
                        Ok((response, results)) => FrontendAttempt::Responded(response, results),
                        Err(e) => FrontendAttempt::CallFailed(e.to_string()),
                    },
                    None => FrontendAttempt::CallFailed("no Response signal arrived".to_string()),
                }
            })();
            let _ = tx.send(outcome);
        });
        rx.recv_timeout(timeout).unwrap_or(FrontendAttempt::TimedOut)
    }

    /// Owns everything one routing-decision case needs: a built real
    /// pick-driver pair, a private Xvfb, a private HOME/XDG environment
    /// and a private activation bus, all freshly created per case so
    /// every ordinary/bypass pair is evaluated from true cold state.
    struct RoutingCase {
        env: PrivatePortalEnv,
        xvfb: PrivateXvfb,
        bus: PrivateActivationBus,
        client: Connection,
    }

    // The descriptor/service files must exist before the private
    // activation `dbus-daemon` is started: a running daemon does not
    // discover a `.service` file that appears in its service directory
    // only after it is already up, so every routing field (the normal
    // bypass defaults, or a refusal's deliberately broken field) has to
    // be written by `write_fn` before `PrivateActivationBus::start` runs.
    fn setup_routing_case(write_fn: impl FnOnce(&PrivatePortalEnv, &Path)) -> (RoutingCase, PathBuf) {
        let (portal_bin, env) = build_portal_activation_artifacts("picker-test-driver-pick");
        let xvfb = PrivateXvfb::start();
        env.activate_private_display(&xvfb.display);
        fs::write(
            env.root().join("home").join("d215-routing-fixture.txt"),
            b"d215 round 215 routing-decision fixture\n",
        )
        .expect("write routing-case fixture file");
        write_fn(&env, &portal_bin);
        let bus = PrivateActivationBus::start(env.round_root());
        let client = bus.connect();
        (
            RoutingCase {
                env,
                xvfb,
                bus,
                client,
            },
            portal_bin,
        )
    }

    fn teardown_routing_case(case: RoutingCase, mut frontend: Child, backend_pid: Option<u32>) {
        drop(case.client);
        terminate_and_reap(&mut frontend);
        if let Some(pid) = backend_pid {
            terminate_and_reap_pid(pid);
        }
        case.bus.finish();
        case.xvfb.finish();
        case.env.finish();
    }

    fn assert_no_false_pick(outcome: &FrontendAttempt) {
        if let FrontendAttempt::Responded(response, results) = outcome {
            assert_ne!(*response, RESPONSE_SUCCESS, "a refused routing decision must never yield a false pick");
            assert!(uris_of(results).is_empty(), "a refused routing decision must carry no uri");
        }
    }

    fn assert_real_pick(outcome: FrontendAttempt, label: &str) {
        match outcome {
            FrontendAttempt::Responded(response, results) => {
                assert_eq!(response, RESPONSE_SUCCESS, "{label}: bypass must restore a real successful pick");
                assert!(!uris_of(&results).is_empty(), "{label}: bypass's successful pick must carry a uri");
            }
            other => panic!("{label}: bypass must restore successful routing, got {other:?}"),
        }
    }

    macro_rules! routing_pair {
        ($refusal_name:ident, $bypass_name:ident, $deny_activation:expr, $break_fn:expr) => {
            #[test]
            fn $refusal_name() {
                let _guard = env_lock();
                let break_fn: fn(&PrivatePortalEnv, &Path) = $break_fn;
                let (case, _portal_bin) = setup_routing_case(|env, portal_bin| break_fn(env, portal_bin));
                assert!(
                    !name_has_owner(&case.client, BUS_NAME),
                    "the backend bus name must have no owner before the first request"
                );
                let frontend = spawn_portal_frontend(&case.xvfb.display, case.bus.address());
                wait_for_name_owner(&case.client, FRONTEND_BUS_NAME, Duration::from_secs(10));
                let outcome = attempt_frontend_open_file_bounded(case.bus.address().to_string(), Duration::from_secs(8));
                assert_no_false_pick(&outcome);
                let deny_activation: bool = $deny_activation;
                let activated = name_has_owner(&case.client, BUS_NAME);
                if deny_activation {
                    assert!(
                        !activated,
                        "this routing decision must make activation of the real backend itself impossible, not just refuse the pick"
                    );
                }
                // Whether or not this decision happens to still activate the
                // backend on its way to refusing (e.g. a real D-Bus method
                // lookup failure discovered only after activation), the
                // activated process -- if any -- is still ours to reap.
                let backend_pid = connection_unix_process_id(&case.client, BUS_NAME);
                teardown_routing_case(case, frontend, backend_pid);
            }

            #[test]
            fn $bypass_name() {
                let _guard = env_lock();
                let (case, _portal_bin) = setup_routing_case(|env, portal_bin| {
                    env.write_portal_descriptor();
                    env.write_service_file(portal_bin);
                });
                assert!(
                    !name_has_owner(&case.client, BUS_NAME),
                    "the backend bus name must have no owner before the first request"
                );
                let frontend = spawn_portal_frontend(&case.xvfb.display, case.bus.address());
                wait_for_name_owner(&case.client, FRONTEND_BUS_NAME, Duration::from_secs(10));
                let outcome = attempt_frontend_open_file_bounded(case.bus.address().to_string(), Duration::from_secs(10));
                let backend_pid = connection_unix_process_id(&case.client, BUS_NAME);
                assert_real_pick(outcome, stringify!($bypass_name));
                teardown_routing_case(case, frontend, backend_pid);
            }
        };
    }

    routing_pair!(
        routing_refusal_descriptor_discovery_missing,
        routing_bypass_descriptor_discovery_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            // No `.portal` descriptor at all: the frontend has nothing
            // naming our interface, so discovery itself must fail closed.
            env.write_service_file(portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_use_in_mismatch,
        routing_bypass_use_in_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            env.write_portal_descriptor_with("UseIn", "SomeOtherUnrelatedTestDesktop");
            env.write_service_file(portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_interfaces_mismatch,
        routing_bypass_interfaces_restored,
        // Observed against the real installed frontend: a descriptor that
        // advertises a different interface can still be cold-activated on
        // the way to discovering the real backend does not expose the
        // requested method there -- the refusal is real (no false pick),
        // the activation is just not where it's prevented.
        false,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            env
                .write_portal_descriptor_with("Interfaces", "org.freedesktop.impl.portal.Notification;");
            env.write_service_file(portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_descriptor_dbus_name_mismatch,
        routing_bypass_descriptor_dbus_name_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            env
                .write_portal_descriptor_with("DBusName", "org.freedesktop.impl.portal.desktop.wrongname");
            env.write_service_file(portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_service_discovery_missing,
        routing_bypass_service_discovery_restored,
        true,
        |env: &PrivatePortalEnv, _portal_bin: &Path| {
            // Descriptor correct, but no `.service` file at all: the bus
            // has nothing to activate even once the backend is selected.
            env.write_portal_descriptor();
        }
    );

    routing_pair!(
        routing_refusal_service_name_mismatch,
        routing_bypass_service_name_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            env.write_portal_descriptor();
            env
                .write_service_file_with("Name", "org.freedesktop.impl.portal.desktop.wrongname", portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_exec_missing_target,
        routing_bypass_exec_missing_target_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            env.write_portal_descriptor();
            env.write_service_file_with(
                "Exec",
                "/nonexistent/d215/wayfinder-portal-missing",
                portal_bin,
            );
        }
    );

    routing_pair!(
        routing_refusal_exec_non_executable_target,
        routing_bypass_exec_non_executable_target_restored,
        true,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            let bogus = env.root().join("not-executable.txt");
            fs::write(&bogus, b"not a real executable\n").expect("write non-executable decoy target");
            env.write_portal_descriptor();
            env.write_service_file_with("Exec", &bogus.display().to_string(), portal_bin);
        }
    );

    routing_pair!(
        routing_refusal_exec_early_dying_target,
        routing_bypass_exec_early_dying_target_restored,
        // An early-dying process may briefly claim the bus name before it
        // exits, so this decision only guarantees no false pick, not that
        // activation itself never happens.
        false,
        |env: &PrivatePortalEnv, portal_bin: &Path| {
            let dying = env.root().join("early-dying.sh");
            fs::write(&dying, b"#!/bin/sh\nexit 7\n").expect("write early-dying decoy target");
            fs::set_permissions(&dying, fs::Permissions::from_mode(0o700))
                .expect("make early-dying decoy executable");
            env.write_portal_descriptor();
            env.write_service_file_with("Exec", &dying.display().to_string(), portal_bin);
        }
    );

    /// A malformed/non-executable/early-dying/missing activation target
    /// must never leave an installed, real backend to answer in its
    /// place: with every real backend hidden behind our own private
    /// `XDG_DESKTOP_PORTAL_DIR`/`XDG_CURRENT_DESKTOP`, the only way an
    /// installed backend could still answer is if the frontend fell back
    /// past our broken descriptor/service to something outside our
    /// private root -- which the refusal cases above already prove does
    /// not happen, since none of them ever yields a real pick.
    #[test]
    fn routing_refusal_never_falls_through_to_installed_backend() {
        let _guard = env_lock();
        let (case, _portal_bin) = setup_routing_case(|env, portal_bin| {
            env.write_portal_descriptor_with("DBusName", "org.freedesktop.impl.portal.desktop.wrongname");
            env.write_service_file(portal_bin);
        });
        assert!(!name_has_owner(&case.client, BUS_NAME));
        let frontend = spawn_portal_frontend(&case.xvfb.display, case.bus.address());
        wait_for_name_owner(&case.client, FRONTEND_BUS_NAME, Duration::from_secs(10));
        let outcome = attempt_frontend_open_file_bounded(case.bus.address().to_string(), Duration::from_secs(8));
        assert_no_false_pick(&outcome);
        assert!(
            !name_has_owner(&case.client, BUS_NAME),
            "our own backend must never be activated by a mismatched descriptor"
        );
        let installed_file_chooser_backend_names = [
            "org.freedesktop.impl.portal.desktop.gtk",
            "org.freedesktop.impl.portal.desktop.kde",
        ];
        for name in installed_file_chooser_backend_names {
            assert!(
                !name_has_owner(&case.client, name),
                "no installed backend may ever be activated on our private bus"
            );
        }
        teardown_routing_case(case, frontend, None);
    }

    /// Round 215 item 7 forced-unwind oracle: panics inside a
    /// `PrivatePortalEnv`'s own scope (before any explicit `finish()`
    /// call), then proves the unwind-safe `Drop` fallback removed the
    /// exact captured root and restored every saved environment variable.
    #[test]
    fn private_portal_env_drop_recovers_root_and_env_on_panic() {
        let _guard = env_lock();
        let old_xdg_current_desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
        let old_display = std::env::var("DISPLAY").ok();
        let captured: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = PrivatePortalEnv::start();
            *captured_clone.lock().unwrap() = Some(env.root().to_path_buf());
            panic!("deliberate panic inside the PrivatePortalEnv fixture scope, before finish()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let root = captured.lock().unwrap().clone().expect("root captured before the panic");
        assert!(!root.exists(), "PrivatePortalEnv::drop must remove its exact root on unwind");
        assert_eq!(
            std::env::var("XDG_CURRENT_DESKTOP").ok(),
            old_xdg_current_desktop,
            "XDG_CURRENT_DESKTOP must be restored after the unwind"
        );
        assert_eq!(
            std::env::var("DISPLAY").ok(),
            old_display,
            "DISPLAY must be restored after the unwind"
        );
    }

    /// Round 215 item 7 forced normal-path cleanup-failure oracle: proves
    /// `PrivatePortalEnv::finish`'s checked removal is actually live.
    #[test]
    fn private_portal_env_finish_panics_when_normal_cleanup_actually_fails() {
        let _guard = env_lock();
        let env = PrivatePortalEnv::start();
        let root = env.root().to_path_buf();
        let blocker = root.join("blocked");
        fs::create_dir(&blocker).expect("create nested dir");
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o000)).expect("lock nested dir");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.finish();
        }));
        assert!(
            result.is_err(),
            "PrivatePortalEnv::finish must panic, not silently swallow, when the checked removal actually fails"
        );
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o700)).expect("unlock nested dir for real cleanup");
        fs::remove_dir_all(&root).expect("real cleanup after the forced-failure oracle");
    }

    /// Round 215 item 7 forced-unwind oracle: panics inside a
    /// `PrivateActivationBus`'s own scope (before any explicit `finish()`
    /// call), then proves the unwind-safe `Drop` fallback reaped the
    /// exact private activation daemon.
    #[test]
    fn private_activation_bus_drop_recovers_daemon_on_panic() {
        let _guard = env_lock();
        let env = PrivatePortalEnv::start();
        let captured: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let bus = PrivateActivationBus::start(env.root());
            *captured_clone.lock().unwrap() = Some(bus.daemon.id());
            panic!("deliberate panic inside the PrivateActivationBus fixture scope, before finish()");
        }));
        assert!(result.is_err(), "the deliberate panic must actually unwind");

        let daemon_pid = captured.lock().unwrap().clone().expect("daemon pid captured before the panic");
        let deadline = Instant::now() + Duration::from_secs(5);
        while PathBuf::from(format!("/proc/{daemon_pid}")).exists() {
            assert!(
                Instant::now() < deadline,
                "PrivateActivationBus::drop must reap the exact daemon process on unwind"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        env.finish();
    }

    /// Round 215 item 7 decoy-survival proof: a file sitting directly
    /// under `~/Projects/Scratch/` that is not owned by any round-215
    /// fixture must survive a full private-env start/finish cycle --
    /// cleanup must only ever touch the exact root it owns, never sweep
    /// its parent directory.
    #[test]
    fn private_portal_env_finish_never_touches_sibling_scratch_entries() {
        let _guard = env_lock();
        let home = std::env::var("HOME").expect("HOME must be set");
        let scratch_root = PathBuf::from(&home).join("Projects/Scratch");
        fs::create_dir_all(&scratch_root).expect("ensure Scratch root exists");
        let decoy = scratch_root.join(format!("d215-decoy-{}", next_unique_id()));
        fs::write(&decoy, b"not owned by any round-215 fixture\n").expect("write decoy sibling file");

        let env = PrivatePortalEnv::start();
        let root = env.root().to_path_buf();
        env.finish();

        assert!(!root.exists(), "the owned root must be gone after finish()");
        assert!(
            decoy.exists(),
            "finish() must never remove a sibling scratch entry it does not own"
        );
        fs::remove_file(&decoy).expect("remove decoy sibling file");
    }
}
