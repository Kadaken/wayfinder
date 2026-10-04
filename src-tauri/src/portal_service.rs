//! Stage-one `org.freedesktop.impl.portal.FileChooser` backend contract
//! for Wayfinder. Exports only `OpenFile`; `SaveFile`/`SaveFiles` are not
//! exported at all. This file owns all service logic; `native.rs` must
//! never import it or call anything here, and this file must never
//! initialize GTK, GStreamer or a display.

use adw::gtk::gio;
use adw::prelude::FileExt;
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

impl Chooser for ProcessChooser {
    fn pick_single(&self, _app_id: &str, _parent_window: &str, _title: &str) -> ChooserOutcome {
        let base = match &self.scratch_base {
            Some(base) => base,
            None => return ChooserOutcome::Failed,
        };
        let root = match RequestRoot::create_under(base) {
            Ok(root) => root,
            Err(_) => return ChooserOutcome::Failed,
        };
        let result_path = root.result_path();

        let child = {
            let mut command = Command::new(&self.executable);
            command
                .arg("--pick")
                .arg("--out")
                .arg(&result_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
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
                    return ChooserOutcome::Picked(vec![PathBuf::from(
                        "test-override-spawn-error-sentinel",
                    )]);
                }
                return ChooserOutcome::Failed;
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

        if self.should_skip_cleanup() {
            // Same completed request, but the cleanup guard removed: do
            // not let `RequestRoot::drop` run, so the exact root that
            // normally vanishes with the outcome instead survives it.
            std::mem::forget(root);
        }

        outcome
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

fn success_result(uri: String) -> (u32, HashMap<String, OwnedValue>) {
    let mut results = HashMap::new();
    let value = Value::from(vec![uri]);
    results.insert(
        "uris".to_string(),
        OwnedValue::try_from(value).expect("Vec<String> always encodes"),
    );
    (RESPONSE_SUCCESS, results)
}

/// Every stage-one option/result refusal is named here so a mutation
/// proof can force exactly one of them off at a time and show the
/// request then wrongly succeeds -- proving that guard, and only that
/// guard, is what was blocking it. Production's one real call site
/// always passes `GuardOverride::None`; nothing reachable from a D-Bus
/// caller, an option value or an environment variable can set anything
/// else, so this is not a backdoor, only a named decision point for
/// tests calling the Rust function directly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuardOverride {
    None,
    SkipMultiple,
    SkipDirectory,
    SkipFilters,
    SkipCurrentFilter,
    SkipChoices,
    SkipCardinality,
    SkipMissingPath,
    SkipInvalidOutput,
}

/// `pick` is called at most once, and only after every option guard has
/// passed -- so a refused request never invokes the chooser seam.
pub fn decide(
    options: &HashMap<String, OwnedValue>,
    pick: impl FnOnce() -> ChooserOutcome,
    override_guard: GuardOverride,
) -> (u32, HashMap<String, OwnedValue>) {
    if override_guard != GuardOverride::SkipMultiple && option_is_true(options, "multiple") {
        return refuse();
    }
    if override_guard != GuardOverride::SkipDirectory && option_is_true(options, "directory") {
        return refuse();
    }
    if override_guard != GuardOverride::SkipFilters && options.contains_key("filters") {
        return refuse();
    }
    if override_guard != GuardOverride::SkipCurrentFilter
        && options.contains_key("current_filter")
    {
        return refuse();
    }
    if override_guard != GuardOverride::SkipChoices && options.contains_key("choices") {
        return refuse();
    }

    match pick() {
        ChooserOutcome::Cancelled => (RESPONSE_CANCELLED, HashMap::new()),
        ChooserOutcome::Failed => refuse(),
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
            if override_guard != GuardOverride::SkipMissingPath
                && candidate.is_absolute()
                && !candidate.is_file()
            {
                return refuse();
            }

            if override_guard == GuardOverride::SkipInvalidOutput {
                let uri = normalize_to_single_file_uri(&candidate)
                    .unwrap_or_else(|| "file:///forced-bypass-proof".to_string());
                return success_result(uri);
            }
            match normalize_to_single_file_uri(&candidate) {
                Some(uri) => success_result(uri),
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
            || chooser.pick_single(&app_id, &parent_window, &title),
            GuardOverride::None,
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
            || chooser.pick_single("app", "", "title"),
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
            || chooser.pick_single("app", "", "title"),
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
            || chooser2.pick_single("app", "", "title"),
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
            || chooser.pick_single("app", "", "title"),
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
            || chooser.pick_single("app", "", "title"),
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
            let owned = OwnedScratchDir::adopt(scratch_base("owned-dir-forced-unwind"));
            fs::create_dir_all(owned.path()).expect("create owned dir");
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
        fs::create_dir_all(&dir).expect("create dir");
        let blocker = dir.join("blocked");
        fs::create_dir(&blocker).expect("create nested dir");
        fs::set_permissions(&blocker, fs::Permissions::from_mode(0o000)).expect("lock nested dir");
        let owned = OwnedScratchDir::adopt(dir.clone());
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

    fn with_present_key(key: &str) -> HashMap<String, OwnedValue> {
        let mut options = HashMap::new();
        options.insert(key.to_string(), OwnedValue::from(0u8));
        options
    }

    // --- decide(): pure guard proofs, no bus involved. ---

    #[test]
    fn decide_success_one_local_path_returns_response_zero_and_one_uri() {
        let fixture = ScratchFile::create();
        let expected_uri = gio::File::for_path(fixture.path()).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
        let (response, results) = decide(&HashMap::new(), || outcome, GuardOverride::None);
        assert_eq!(response, RESPONSE_SUCCESS);
        let uris = uris_of(&results);
        assert_eq!(uris, vec![expected_uri]);
        fixture.remove();
    }

    #[test]
    fn decide_cancelled_returns_response_one_and_no_uri() {
        let (response, results) = decide(&HashMap::new(), || ChooserOutcome::Cancelled, GuardOverride::None);
        assert_eq!(response, RESPONSE_CANCELLED);
        assert!(results.is_empty());
    }

    #[test]
    fn decide_failed_returns_response_other_and_no_uri() {
        let (response, results) = decide(&HashMap::new(), || ChooserOutcome::Failed, GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
        assert!(results.is_empty());
    }

    macro_rules! guard_proof {
        ($red:ident, $bypassed:ident, $options:expr, $override:expr) => {
            #[test]
            fn $red() {
                let fixture = ScratchFile::create();
                let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
                let (response, _) = decide(&$options, || outcome, GuardOverride::None);
                assert_eq!(response, RESPONSE_OTHER);
                fixture.remove();
            }

            #[test]
            fn $bypassed() {
                let fixture = ScratchFile::create();
                let outcome = ChooserOutcome::Picked(vec![fixture.path().to_path_buf()]);
                let (response, _) = decide(&$options, || outcome, $override);
                assert_eq!(
                    response, RESPONSE_SUCCESS,
                    "bypassing this guard must turn the request's own refusal into success"
                );
                fixture.remove();
            }
        };
    }

    guard_proof!(
        decide_refuses_multiple_true,
        decide_bypassing_multiple_guard_then_succeeds,
        with_option("multiple", true),
        GuardOverride::SkipMultiple
    );
    guard_proof!(
        decide_refuses_directory_true,
        decide_bypassing_directory_guard_then_succeeds,
        with_option("directory", true),
        GuardOverride::SkipDirectory
    );
    guard_proof!(
        decide_refuses_filters_present,
        decide_bypassing_filters_guard_then_succeeds,
        with_present_key("filters"),
        GuardOverride::SkipFilters
    );
    guard_proof!(
        decide_refuses_current_filter_present,
        decide_bypassing_current_filter_guard_then_succeeds,
        with_present_key("current_filter"),
        GuardOverride::SkipCurrentFilter
    );
    guard_proof!(
        decide_refuses_choices_present,
        decide_bypassing_choices_guard_then_succeeds,
        with_present_key("choices"),
        GuardOverride::SkipChoices
    );

    #[test]
    fn decide_refuses_more_than_one_picked_path() {
        let fixture = ScratchFile::create();
        let outcome = ChooserOutcome::Picked(vec![
            fixture.path().to_path_buf(),
            PathBuf::from("/nonexistent-round212-path/second.txt"),
        ]);
        let (response, _) = decide(&HashMap::new(), || outcome, GuardOverride::None);
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
        let (response, results) = decide(&HashMap::new(), || outcome, GuardOverride::SkipCardinality);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec![expected_uri], "only the first path is normalized");
        fixture.remove();
    }

    #[test]
    fn decide_refuses_non_local_output() {
        let outcome = ChooserOutcome::Picked(vec![PathBuf::from("relative/not-absolute.txt")]);
        let (response, _) = decide(&HashMap::new(), || outcome, GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
    }

    #[test]
    fn decide_bypassing_invalid_output_guard_then_succeeds_on_non_local_path() {
        let outcome = ChooserOutcome::Picked(vec![PathBuf::from("relative/not-absolute.txt")]);
        let (response, results) =
            decide(&HashMap::new(), || outcome, GuardOverride::SkipInvalidOutput);
        assert_eq!(response, RESPONSE_SUCCESS);
        assert_eq!(uris_of(&results), vec!["file:///forced-bypass-proof".to_string()]);
    }

    #[test]
    fn decide_refuses_missing_absolute_path() {
        let outcome =
            ChooserOutcome::Picked(vec![PathBuf::from("/nonexistent-round212-path/missing.txt")]);
        let (response, results) = decide(&HashMap::new(), || outcome, GuardOverride::None);
        assert_eq!(response, RESPONSE_OTHER);
        assert!(uris_of(&results).is_empty());
    }

    #[test]
    fn decide_bypassing_missing_path_guard_then_succeeds_on_missing_file() {
        let missing = PathBuf::from("/nonexistent-round212-path/missing.txt");
        let expected_uri = gio::File::for_path(&missing).uri().to_string();
        let outcome = ChooserOutcome::Picked(vec![missing]);
        let (response, results) =
            decide(&HashMap::new(), || outcome, GuardOverride::SkipMissingPath);
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
            decide(&HashMap::new(), || outcome, GuardOverride::SkipMissingPath);
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
            decide(&HashMap::new(), || outcome, GuardOverride::SkipInvalidOutput);
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

        let (response, _) = call_open_file(&client, with_option("multiple", true)).expect("OpenFile call");
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

    dbus_option_refusal!(dbus_openfile_refuses_multiple_true, with_option("multiple", true));
    dbus_option_refusal!(dbus_openfile_refuses_directory_true, with_option("directory", true));
    dbus_option_refusal!(dbus_openfile_refuses_filters_present, with_present_key("filters"));
    dbus_option_refusal!(
        dbus_openfile_refuses_current_filter_present,
        with_present_key("current_filter")
    );
    dbus_option_refusal!(dbus_openfile_refuses_choices_present, with_present_key("choices"));

    #[test]
    fn dbus_savefile_is_an_unknown_method() {
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
                "SaveFile",
                &(),
            )
            .expect_err("SaveFile is never exported");
        match err {
            zbus::Error::MethodError(name, ..) => {
                assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.UnknownMethod");
            }
            other => panic!("expected UnknownMethod, got {other:?}"),
        }
        finish_private_bus(bus, server, client);
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
        assert!(!xml.contains("SaveFile\""), "SaveFile must not be exported");
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
        fn adopt(dir: PathBuf) -> Self {
            Self {
                dir,
                removed: Cell::new(false),
            }
        }

        fn path(&self) -> &Path {
            &self.dir
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
    fn build_picker_test_driver(feature: &str) -> (PathBuf, PathBuf) {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let target_dir = scratch_base(&format!("native-driver-target-{feature}"));
        let status = Command::new("cargo")
            .arg("build")
            .arg("--quiet")
            .arg("--bin")
            .arg("wayfinder-native")
            .arg("--features")
            .arg(feature)
            .arg("--target-dir")
            .arg(&target_dir)
            .current_dir(manifest_dir)
            .status()
            .expect("run cargo build for the test-only native driver");
        assert!(status.success(), "building the {feature} native driver failed");
        let binary = target_dir.join("debug").join("wayfinder-native");
        assert!(binary.exists(), "expected driver binary at {}", binary.display());
        (binary, target_dir)
    }

    #[test]
    fn native_picker_real_pick_through_exported_open_file() {
        let _guard = env_lock();
        let (driver, driver_target) = build_picker_test_driver("picker-test-driver-pick");
        let driver_target = OwnedScratchDir::adopt(driver_target);
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        // Both of these read the real `$HOME` to build their own scratch
        // roots, so they must run before `$HOME` is overridden below.
        let bus = PrivateBus::start();
        let base = scratch_base("native-pick-base");
        let fixture_dir_root = OwnedScratchDir::adopt(scratch_base("native-pick-fixture-home"));
        fs::create_dir_all(fixture_dir_root.path()).expect("create fixture HOME dir");
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
        let driver_target = OwnedScratchDir::adopt(driver_target);
        let xvfb = PrivateXvfb::start();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();
        std::env::set_var("DISPLAY", &xvfb.display);
        std::env::remove_var("WAYLAND_DISPLAY");

        let bus = PrivateBus::start();
        let base = scratch_base("native-cancel-base");
        let fixture_dir_root = OwnedScratchDir::adopt(scratch_base("native-cancel-fixture-home"));
        fs::create_dir_all(fixture_dir_root.path()).expect("create fixture HOME dir");
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
}
