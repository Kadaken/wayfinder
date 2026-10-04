//! Stage-one `org.freedesktop.impl.portal.FileChooser` backend contract
//! for Wayfinder. Exports only `OpenFile`; `SaveFile`/`SaveFiles` are not
//! exported at all. This file owns all service logic; `native.rs` must
//! never import it or call anything here, and this file must never
//! initialize GTK, GStreamer or a display.

use adw::gtk::gio;
use adw::prelude::FileExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// Stage-one production default: no real Wayfinder picker child exists
/// yet (that is stage two), so every request fails closed. No hidden
/// environment variable, owner path, test file, synthetic click or
/// command-line backdoor may change this in a production process --
/// tests inject their own `Chooser` directly when they register the
/// service object, rather than steering this one.
pub struct UnavailableChooser;

impl Chooser for UnavailableChooser {
    fn pick_single(&self, _app_id: &str, _parent_window: &str, _title: &str) -> ChooserOutcome {
        ChooserOutcome::Failed
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
            // before URI normalization, so a relative/non-local refusal can
            // never incidentally cover for it (or vice versa).
            if override_guard != GuardOverride::SkipMissingPath
                && override_guard != GuardOverride::SkipInvalidOutput
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
            std::env::set_var("TMPDIR", &root);
            std::env::set_var("XDG_RUNTIME_DIR", &run_dir);

            Self {
                root,
                address,
                daemon,
                old_tmpdir,
                old_xdg_runtime_dir,
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
}
