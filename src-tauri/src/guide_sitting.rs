//! Guide sitting: Wayfinder's adapter for the shared app-owned attached-card
//! protocol (D-154). Guide builds only.
//!
//! The protocol half (`Sitting`, `Adapter`) is toolkit-neutral and mirrors the
//! shared Python protocol (`~/bin/guide_sitting.py`) rule for rule: the record
//! schema, one Detach, abandon-on-outside-close, and the canonical exchange
//! (the app writes `<descriptor>.request`; checkrun's ONE canonical writer
//! replies in `<descriptor>.reply`; the app closes only after it has found the
//! exact file and matched its sha256 and the lifecycle record is written).
//! Every adapter callback fails closed: `Ok` only when it really happened.
//!
//! The card contract is data: `checkrun --attached` writes
//! `<descriptor>.session` (checkrun_core.session_payload) and the GTK card
//! renders exactly that: numbered outcomes, hops, Works/Broken, per-step Show
//! me where, Worked/Broken/Skipped with no default, notes, screenshots, copy
//! helpers, way back, saved progress and already-answered protection. No
//! answers file is written here.
//!
//! Descriptor/ack verification is NOT re-implemented in Rust: every check
//! calls the one shared verifier (`guide_sitting.py verify`), and refuses if
//! it cannot run it.
//!
//! Left placement uses the X11 backend the guide builds run on: the parent's
//! client origin comes from XTranslateCoordinates on the parent's own
//! surface, the card is moved with XMoveWindow. GTK4 has no move/position
//! event, so the coupling is a 40 ms poll of the parent's origin and size, and
//! of the card's own origin. On any other backend the sitting is refused.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CARD_W: i32 = 460;
const CARD_H: i32 = 640;
const POLL_MS: u64 = 40;
/// A move is asked for again every poll while the card's observed origin
/// disagrees with the desired seam. Past this many consecutive unmatched
/// polls (~1 s) a mismatch is treated as a permanently refused axis and the
/// poll stops re-issuing `XMoveWindow`, rather than storming it forever.
const MAX_PLACE_STALL: u32 = 25;

/// Every callback returns Ok only when the thing really happened.
pub trait Adapter {
    fn create_card(&mut self, app_key: &str) -> Result<String, String>;
    fn place_card_left(&mut self, app_key: &str, card_key: &str) -> Result<(), String>;
    fn couple(&mut self, app_key: &str, card_key: &str) -> Result<(), String>;
    fn uncouple(&mut self, card_key: &str) -> Result<(), String>;
    fn mark(&mut self, app_key: &str, control: &str) -> Result<(), String>;
    fn clear_marks(&mut self, app_key: &str) -> Result<(), String>;
    fn close_card(&mut self, card_key: &str) -> Result<(), String>;
    fn close_app(&mut self, app_key: &str) -> Result<(), String>;
    /// Clear any bounded-refusal state left over from a prior follow, so the
    /// next `place_card_left` starts a fresh bounded acknowledgement instead
    /// of immediately refusing again on the same, still-unchanged desired
    /// seam. A no-op by default; adapters that track refusal override it.
    fn reset_follow(&mut self, _app_key: &str, _card_key: &str) {}
}

#[derive(Debug)]
pub struct SittingError(#[allow(dead_code)] pub String);

pub fn sitting_path(descriptor: &Path) -> PathBuf {
    side_path(descriptor, "sitting")
}

pub fn side_path(descriptor: &Path, ext: &str) -> PathBuf {
    let mut n = descriptor.file_name().unwrap_or_default().to_os_string();
    n.push(format!(".{ext}"));
    descriptor.with_file_name(n)
}

/// Write `text` to `path` mode 0600 through a temp file moved into place.
pub fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".tmp{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn file_sha256(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The one shared verifier, run as `guide_sitting.py verify …`. Any failure to
/// run it is a refusal.
pub fn run_verifier(descriptor: &Path, request_path: &str, checklist: &Value, token: &str) -> Result<(), String> {
    let script = std::env::var_os("WF_GUIDE_VERIFIER").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("bin/guide_sitting.py")
    });
    if !script.is_file() {
        return Err(format!("shared verifier not found at {}", script.display()));
    }
    let out = std::process::Command::new("python3")
        .arg(&script)
        .arg("verify")
        .arg(descriptor)
        .args(["--request-path", request_path])
        .args(["--checklist", checklist["path"].as_str().unwrap_or("")])
        .args(["--sha", checklist["sha256"].as_str().unwrap_or("")])
        .args(["--window-token", token])
        .output()
        .map_err(|e| format!("shared verifier would not run: {e}"))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "shared verifier gave no answer".to_string())?;
    if v["ok"] == Value::Bool(true) {
        Ok(())
    } else {
        Err(v["reason"].as_str().unwrap_or("refused").to_string())
    }
}

struct Pending {
    nonce: String,
    action: String,
    deadline: Instant,
}

pub struct Sitting<A: Adapter> {
    pub adapter: A,
    descriptor: PathBuf,
    token: String,
    app_key: String,
    card_key: Option<String>,
    checklist: Value,
    answers_path: String,
    progress_path: String,
    verify: Box<dyn Fn() -> Result<(), String>>,
    claimed_by: Box<dyn Fn(&str) -> bool>,
    pub state: Option<String>,
    pub lifecycle: Option<String>,
    pub outcome: Option<String>,
    /// Windows this sitting still owns ("card", "app"); recorded so a
    /// surviving window is never untracked.
    pub owned: Vec<String>,
    pub error: String,
    /// Reality changed and the record could not follow; written when possible.
    dirty: bool,
    #[cfg(test)]
    writes: std::cell::Cell<usize>,
    #[cfg(test)]
    fail_at: std::cell::RefCell<Vec<usize>>,
    pending: Option<Pending>,
    timeout: Duration,
}

impl<A: Adapter> Sitting<A> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: A,
        descriptor: PathBuf,
        token: String,
        app_key: String,
        checklist: Value,
        answers_path: String,
        progress_path: String,
        verify: Box<dyn Fn() -> Result<(), String>>,
        claimed_by: Box<dyn Fn(&str) -> bool>,
    ) -> Self {
        Sitting {
            adapter,
            descriptor,
            token,
            app_key,
            card_key: None,
            checklist,
            answers_path,
            progress_path,
            verify,
            claimed_by,
            state: None,
            lifecycle: None,
            outcome: None,
            owned: Vec::new(),
            error: String::new(),
            dirty: false,
            #[cfg(test)]
            writes: std::cell::Cell::new(0),
            #[cfg(test)]
            fail_at: std::cell::RefCell::new(Vec::new()),
            pending: None,
            timeout: Duration::from_secs(30),
        }
    }

    #[allow(dead_code)]
    pub fn set_timeout(&mut self, t: Duration) {
        self.timeout = t;
    }

    pub fn is_attached(&self) -> bool {
        self.lifecycle.as_deref() == Some("open") && self.state.as_deref() == Some("attached")
    }

    pub fn is_open(&self) -> bool {
        self.lifecycle.as_deref() == Some("open")
    }

    /// Open, or closing with an owned window still to close.
    pub fn is_tracked(&self) -> bool {
        matches!(self.lifecycle.as_deref(), Some("open") | Some("closing"))
    }

    fn checklist_path(&self) -> PathBuf {
        PathBuf::from(self.checklist["path"].as_str().unwrap_or(""))
    }

    fn record(&mut self) -> std::io::Result<()> {
        let r = self.write_record();
        if r.is_ok() {
            self.dirty = false;
        }
        r
    }

    /// Write the record as it now is (state, owned, error) if reality moved
    /// without it. False while it still cannot be written.
    pub fn reconcile(&mut self) -> bool {
        if self.dirty {
            return self.record().is_ok();
        }
        true
    }

    fn write_record(&self) -> std::io::Result<()> {
        #[cfg(test)]
        {
            self.writes.set(self.writes.get() + 1);
            if self.fail_at.borrow().contains(&self.writes.get()) {
                return Err(std::io::Error::other("injected write failure"));
            }
        }
        atomic_write(
            &sitting_path(&self.descriptor),
            &json!({
                "token": self.token,
                "app_window_key": self.app_key,
                "card_window_key": self.card_key,
                "checklist": self.checklist,
                "state": self.state,
                "lifecycle": self.lifecycle,
                "outcome": self.outcome,
                "owned": self.owned,
                "error": self.error,
            })
            .to_string(),
        )
    }

    /// Record first; keep the old values if the write fails.
    fn transition(&mut self, state: Option<&str>, lifecycle: Option<&str>) -> bool {
        self.transition_outcome(state, lifecycle, None)
    }

    fn transition_outcome(&mut self, state: Option<&str>, lifecycle: Option<&str>, outcome: Option<&str>) -> bool {
        let old = (self.state.clone(), self.lifecycle.clone(), self.outcome.clone());
        if let Some(o) = outcome {
            self.outcome = Some(o.into());
        }
        if let Some(s) = state {
            self.state = Some(s.into());
        }
        if let Some(l) = lifecycle {
            self.lifecycle = Some(l.into());
        }
        if let Err(e) = self.record() {
            self.state = old.0;
            self.lifecycle = old.1;
            self.outcome = old.2;
            self.error = format!("sitting record not written: {e}");
            return false;
        }
        true
    }

    /// Keep the error in memory and, where the record is writable, on disk.
    /// If it is not writable the original error is what survives.
    fn note_error(&mut self, why: String) -> bool {
        self.error = why;
        if self.record().is_err() {
            self.dirty = true;
        }
        false
    }

    /// Record "closing" first (durable, naming the owned windows), close what
    /// is owned, then record the clean outcome. A close failure leaves the
    /// sitting "closing": tracked and retryable.
    fn finish(&mut self, outcome: &str) -> bool {
        if !self.transition_outcome(None, Some("closing"), Some(outcome)) {
            return false;
        }
        self.close_owned()
    }

    fn close_owned(&mut self) -> bool {
        let mut errs = Vec::new();
        if self.owned.iter().any(|w| w == "card") {
            if let Some(card) = self.card_key.clone() {
                match self.adapter.close_card(&card) {
                    Ok(()) => {
                        self.owned.retain(|w| w != "card");
                        self.dirty = true;
                    }
                    Err(e) => errs.push(format!("card: {e}")),
                }
            } else {
                self.owned.retain(|w| w != "card");
            }
        }
        if self.owned.iter().any(|w| w == "app") {
            let app = self.app_key.clone();
            match self.adapter.close_app(&app) {
                Ok(()) => {
                    self.owned.retain(|w| w != "app");
                    self.dirty = true;
                }
                Err(e) => errs.push(format!("app: {e}")),
            }
        }
        if !errs.is_empty() {
            return self.note_error(format!("close failed: {}", errs.join("; ")));
        }
        let outcome = self.outcome.clone();
        self.error.clear();
        if !self.transition(None, outcome.as_deref()) {
            self.dirty = true; // closes happened; record still says otherwise
            return false;
        }
        true
    }

    /// Close whatever a failed close left behind, then record the outcome.
    pub fn retry_close(&mut self) -> bool {
        self.reconcile();
        if self.lifecycle.as_deref() != Some("closing") {
            return false;
        }
        self.close_owned()
    }

    fn live(&self) -> bool {
        (self.verify)().is_ok() && (self.claimed_by)(&self.app_key)
    }

    fn refuse(&mut self, why: String) -> SittingError {
        self.error = why;
        self.lifecycle = Some("refused".into());
        self.owned.clear();
        if let Err(e) = self.record() {
            self.error = format!("{}; sitting record not written: {e}", self.error);
        }
        SittingError(self.error.clone())
    }

    fn teardown_card(&mut self) {
        if let Some(k) = self.card_key.take() {
            let _ = self.adapter.close_card(&k);
            self.owned.retain(|w| w != "card");
        }
    }

    pub fn open(&mut self) -> Result<(), SittingError> {
        if !(self.claimed_by)(&self.app_key) {
            return Err(self.refuse("window did not claim the descriptor token".into()));
        }
        if let Err(why) = (self.verify)() {
            return Err(self.refuse(format!("descriptor/ack refused: {why}")));
        }
        if file_sha256(&self.checklist_path()).as_deref() != self.checklist["sha256"].as_str() {
            return Err(self.refuse("checklist identity does not match the session".into()));
        }
        let app = self.app_key.clone();
        let attach = (|| -> Result<(), String> {
            let card = self.adapter.create_card(&app)?;
            self.card_key = Some(card.clone());
            self.owned = vec!["card".into(), "app".into()];
            self.adapter.place_card_left(&app, &card)?;
            self.adapter.couple(&app, &card)
        })();
        if let Err(e) = attach {
            self.teardown_card();
            return Err(self.refuse(format!("cannot attach the card: {e}")));
        }
        self.state = Some("attached".into());
        self.lifecycle = Some("open".into());
        if let Err(e) = self.record() {
            self.teardown_card();
            self.state = None;
            self.lifecycle = None;
            return Err(self.refuse(format!("sitting record not written: {e}")));
        }
        Ok(())
    }

    pub fn parent_geometry_changed(&mut self) -> bool {
        self.reconcile();
        if self.is_attached() {
            if let Some(card) = self.card_key.clone() {
                let app = self.app_key.clone();
                if let Err(e) = self.adapter.place_card_left(&app, &card) {
                    return self.follow_refused(format!("follow failed: {e}"));
                }
            }
        }
        true
    }

    /// A follow (`place_card_left`) came back with a terminal `Err`: the
    /// card's observed origin will not reach the desired seam. The record
    /// must never go on saying "attached" after this, and the shared schema
    /// (`guide_sitting.py`) only knows `attached`/`detached` — there is no
    /// third state, so this moves to the existing `detached` value, exactly
    /// as `detach()` does, rather than inventing a schema-invalid one.
    /// Coupling is stopped (the poll that kept re-driving a refused move),
    /// matching the meaning of "detached". State is moved off "attached" in
    /// memory FIRST, then the record write is attempted; a write failure
    /// leaves memory correctly non-attached anyway (marked `dirty` for a
    /// later `reconcile`) instead of reverting to "attached" and inventing a
    /// success that did not happen. `owned` is untouched so the card and app
    /// windows this sitting still owns stay tracked for an exact close.
    /// Reattach clears the adapter's own refusal/stall bookkeeping before
    /// trying again, so this refusal does not repeat forever.
    fn follow_refused(&mut self, why: String) -> bool {
        self.error = why;
        if let Some(card) = self.card_key.clone() {
            let _ = self.adapter.uncouple(&card);
        }
        self.state = Some("detached".into());
        if self.record().is_err() {
            self.dirty = true;
        }
        false
    }

    pub fn detach(&mut self) -> bool {
        self.reconcile();
        if !self.is_attached() {
            return false;
        }
        let card = self.card_key.clone().unwrap_or_default();
        if let Err(e) = self.adapter.uncouple(&card) {
            return self.note_error(format!("detach failed: {e}"));
        }
        if self.transition(Some("detached"), None) {
            return true;
        }
        // The record would not hold, so the sitting must stay attached in fact
        // as well as in the record: put the coupling back.
        let why = self.error.clone();
        let app = self.app_key.clone();
        let back = self
            .adapter
            .place_card_left(&app, &card)
            .and_then(|_| self.adapter.couple(&app, &card));
        if let Err(e) = back {
            // Coupling is really gone; say so rather than claim attached.
            self.state = Some("detached".into());
            self.note_error(format!("{why}; and re-coupling failed: {e}"));
        }
        false
    }

    /// Reverse of `detach()`: only from an open, detached sitting, only back
    /// onto the same still-claimed app. Fails closed with state matching
    /// reality; a failed record undoes the coupling so nothing is coupled
    /// that the record does not say is coupled.
    pub fn reattach(&mut self) -> bool {
        self.reconcile();
        if !self.is_open() || self.state.as_deref() != Some("detached") {
            return false;
        }
        if !(self.claimed_by)(&self.app_key) {
            return self.note_error("window did not claim the descriptor token".into());
        }
        if let Err(why) = (self.verify)() {
            return self.note_error(format!("descriptor/ack refused: {why}"));
        }
        let Some(card) = self.card_key.clone() else {
            return self.note_error("no card to reattach".into());
        };
        let app = self.app_key.clone();
        // A prior follow may have hit the bounded refusal and left the
        // adapter's own retry bookkeeping in the refused state; clear it so
        // this reattach gets a fresh bounded acknowledgement rather than
        // failing again immediately on the same still-unchanged seam.
        self.adapter.reset_follow(&app, &card);
        if let Err(e) = self.adapter.place_card_left(&app, &card) {
            return self.note_error(format!("reattach failed: {e}"));
        }
        if let Err(e) = self.adapter.couple(&app, &card) {
            return self.note_error(format!("reattach failed: {e}"));
        }
        if self.transition(Some("attached"), None) {
            return true;
        }
        // The record would not hold: reality must not claim coupling the
        // record does not have. Undo it and report the original failure.
        let why = self.error.clone();
        if let Err(e) = self.adapter.uncouple(&card) {
            return self.note_error(format!("{why}; and uncoupling failed: {e}"));
        }
        self.error = why;
        false
    }

    pub fn show_me_where(&mut self, control: &str) -> bool {
        self.reconcile();
        if !self.is_open() || !self.live() {
            return false;
        }
        let app = self.app_key.clone();
        match self.adapter.mark(&app, control) {
            Ok(()) => true,
            Err(e) => self.note_error(format!("mark failed: {e}")),
        }
    }

    pub fn clear_marks(&mut self) -> bool {
        self.reconcile();
        if self.is_open() {
            let app = self.app_key.clone();
            if let Err(e) = self.adapter.clear_marks(&app) {
                return self.note_error(format!("clear failed: {e}"));
            }
        }
        true
    }

    /// Ask checkrun's canonical writer to act (submit | save | back). False =
    /// refused, nothing written. Then poll() until it is not None.
    pub fn begin(&mut self, action: &str, results: &Value) -> bool {
        if !self.is_open() || self.pending.is_some() || !["submit", "save", "back"].contains(&action) {
            return false;
        }
        if file_sha256(&self.checklist_path()).as_deref() != self.checklist["sha256"].as_str() {
            self.error = "checklist changed since the sitting opened".into();
            return false;
        }
        if !self.live() {
            self.error = "sitting no longer verifies".into();
            return false;
        }
        let nonce: String = {
            let mut b = [0u8; 8];
            if let Ok(mut f) = fs::File::open("/dev/urandom") {
                use std::io::Read;
                let _ = f.read_exact(&mut b);
            }
            b.iter().map(|x| format!("{x:02x}")).collect()
        };
        let _ = fs::remove_file(side_path(&self.descriptor, "reply"));
        let body = json!({"nonce": nonce, "token": self.token, "action": action,
                          "results": results, "checklist": self.checklist});
        if let Err(e) = atomic_write(&side_path(&self.descriptor, "request"), &body.to_string()) {
            self.error = format!("request not written: {e}");
            return false;
        }
        self.pending = Some(Pending { nonce, action: action.into(), deadline: Instant::now() + self.timeout });
        true
    }

    fn clear_exchange(&mut self) {
        let _ = fs::remove_file(side_path(&self.descriptor, "request"));
        let _ = fs::remove_file(side_path(&self.descriptor, "reply"));
        self.pending = None;
    }

    /// None while waiting; Some(true) done; Some(false) failed (nothing
    /// closed, sitting still open).
    pub fn poll(&mut self) -> Option<bool> {
        let (nonce, action, deadline) = match &self.pending {
            Some(p) => (p.nonce.clone(), p.action.clone(), p.deadline),
            None => return Some(false),
        };
        let reply: Option<Value> = fs::read_to_string(side_path(&self.descriptor, "reply"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let Some(reply) = reply.filter(|r| r["nonce"].as_str() == Some(nonce.as_str())) else {
            if Instant::now() > deadline {
                self.error = "checkrun did not answer in time".into();
                self.clear_exchange();
                return Some(false);
            }
            return None;
        };
        if reply["ok"] != Value::Bool(true) {
            self.error = reply["error"].as_str().unwrap_or("checkrun refused").to_string();
            self.clear_exchange();
            return Some(false);
        }
        if action == "back" {
            self.clear_exchange();
            return Some(true);
        }
        let want = if action == "submit" { self.answers_path.clone() } else { self.progress_path.clone() };
        let got = reply["answers_path"].as_str().unwrap_or("").to_string();
        let sha = file_sha256(Path::new(&got));
        if want.is_empty() || got != want || sha.is_none() || sha.as_deref() != reply["sha256"].as_str() {
            self.error = "the canonical file is not the one expected".into();
            self.clear_exchange();
            return Some(false);
        }
        let final_state = if action == "submit" { "submitted" } else { "saved" };
        self.clear_exchange();
        Some(self.finish(final_state))
    }

    /// A window was closed by the user outside Submit -> abandon; both windows
    /// end closed (the closed one idempotently).
    pub fn window_closed(&mut self) -> bool {
        self.reconcile();
        if !self.is_open() {
            return false;
        }
        self.clear_exchange();
        // Durable first: if the record will not hold, the other window is NOT
        // closed and the sitting stays open and tracked.
        self.finish("abandoned")
    }
}

// ---- GTK4 adapter (adapter one) --------------------------------------------

use super::{guide_apply, guide_claimed_by, guide_descriptor_path, ui_for_state, unregister_window, AppState, GuideRequest};
use adw::gtk;
use adw::prelude::*;
use adw::gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::os::raw::{c_int, c_ulong, c_void};
use std::rc::{Rc, Weak};

extern "C" {
    fn gdk_x11_display_get_xdisplay(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
    #[link_name = "gdk_x11_surface_get_xid"]
    fn sitting_surface_xid(surface: *mut gdk::ffi::GdkSurface) -> c_ulong;
}
#[link(name = "X11")]
extern "C" {
    fn XDefaultRootWindow(d: *mut c_void) -> c_ulong;
    fn XTranslateCoordinates(
        d: *mut c_void, src: c_ulong, dst: c_ulong, sx: c_int, sy: c_int,
        dx: *mut c_int, dy: *mut c_int, child: *mut c_ulong,
    ) -> c_int;
    fn XMoveWindow(d: *mut c_void, w: c_ulong, x: c_int, y: c_int) -> c_int;
    #[cfg(test)]
    fn XResizeWindow(d: *mut c_void, w: c_ulong, width: std::os::raw::c_uint, height: std::os::raw::c_uint) -> c_int;
    fn XFlush(d: *mut c_void) -> c_int;
}

/// (display, xid) of a realized toplevel on the X11 backend, else None.
fn x11_handle(window: &impl IsA<gtk::Native>) -> Option<(*mut c_void, c_ulong)> {
    use glib::translate::ToGlibPtr;
    let surface = gtk::prelude::NativeExt::surface(window.upcast_ref::<gtk::Native>())?;
    let display = surface.display();
    if !display.type_().name().contains("X11") {
        return None;
    }
    unsafe {
        let d = gdk_x11_display_get_xdisplay(display.to_glib_none().0);
        let xid = sitting_surface_xid(surface.to_glib_none().0);
        if d.is_null() || xid == 0 { None } else { Some((d, xid)) }
    }
}

/// Root-relative origin of a window's client area, plus its size.
pub fn window_geometry(window: &(impl IsA<gtk::Native> + IsA<gtk::Widget>)) -> Option<(i32, i32, i32, i32)> {
    let (d, xid) = x11_handle(window)?;
    let (mut x, mut y, mut child) = (0 as c_int, 0 as c_int, 0 as c_ulong);
    unsafe {
        if XTranslateCoordinates(d, xid, XDefaultRootWindow(d), 0, 0, &mut x, &mut y, &mut child) == 0 {
            return None;
        }
    }
    Some((x, y, window.width(), window.height()))
}

/// Test seam: resize a realized toplevel with a real X11 request, like a
/// window-manager or user resize (GTK4 default size does nothing once mapped).
#[cfg(test)]
pub fn x11_resize(window: &impl IsA<gtk::Native>, w: u32, h: u32) {
    if let Some((d, xid)) = x11_handle(window) {
        unsafe {
            XResizeWindow(d, xid, w, h);
            XFlush(d);
        }
    }
}

/// Test seam: move any realized toplevel to (x, y) with the same X11 call the
/// card uses, so a proof can move the parent like a real drag would.
#[cfg(test)]
pub fn x11_move(window: &impl IsA<gtk::Native>, x: i32, y: i32) {
    if let Some((d, xid)) = x11_handle(window) {
        unsafe {
            XMoveWindow(d, xid, x, y);
            XFlush(d);
        }
    }
}


type SittingCell = Rc<RefCell<Option<Weak<RefCell<Sitting<GtkAdapter>>>>>>;

/// Run `f` on the live sitting. try_borrow_mut: a press or close caused by
/// the sitting's own teardown arrives while it is borrowed and is ignored.
fn with_sitting<R>(cell: &SittingCell, f: impl FnOnce(&mut Sitting<GtkAdapter>) -> R) -> Option<R> {
    let rc = cell.borrow().as_ref().and_then(|w| w.upgrade())?;
    let mut guard = rc.try_borrow_mut().ok()?;
    Some(f(&mut guard))
}

const VERDICT_NAMES: [&str; 3] = ["Worked", "Broken", "Skipped"];
const CARD_CSS: &str = "\
.guide-card-ring { outline: 4px solid #e8590c; outline-offset: 2px; }
.guide-step-worked { outline: 2px solid #3fb950; outline-offset: 2px; }
.guide-step-broken { outline: 2px solid #f0883e; outline-offset: 2px; }
.guide-step-skipped { outline: 2px solid #8b949e; outline-offset: 2px; }
";
const NOTES_PLACEHOLDER: &str = "What you saw (only if broken, or worth saying)";

/// Everything one step draws and answers with.
pub struct StepUi {
    pub n: Value,
    pub tag: String,
    pub show: Option<gtk::Button>,
    pub toggles: Vec<gtk::CheckButton>,
    pub copy_buttons: Vec<gtk::Button>,
    pub notes: gtk::TextView,
    pub notes_placeholder: gtk::Label,
    pub copy_notes: gtk::Button,
    pub grab: gtk::Button,
    pub shot_rows: gtk::Box,
    pub shots: RefCell<Vec<PathBuf>>,
    pub remove_buttons: RefCell<Vec<gtk::Button>>,
    pub frame: gtk::Box,
    pub head: gtk::Label,
    head_text: String,
    copy_texts: Vec<String>,
    was: RefCell<String>,
    locked: bool,
    guard: Cell<bool>,
}

/// The app-owned card, rendered from the checkrun session payload.
pub struct CardUi {
    pub window: gtk::Window,
    pub payload: Value,
    pub steps: Vec<StepUi>,
    pub detach: gtk::Button,
    pub back: gtk::Button,
    pub back_says: gtk::Label,
    pub notice: gtk::Label,
    pub save: gtk::Button,
    pub submit: gtk::Button,
    pub marked: Cell<Option<usize>>,
    busy: Cell<bool>,
    building: Cell<bool>,
    confirm_skip: Cell<bool>,
    pub attached: Cell<bool>,
    cell: SittingCell,
}

fn n_str(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
}

fn label(text: &str, wrap: bool) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(wrap);
    l.set_selectable(false);
    l
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

impl CardUi {
    fn verdict_of(&self, i: usize) -> Option<&'static str> {
        let st = &self.steps[i];
        st.toggles.iter().position(|t| t.is_active()).map(|p| VERDICT_NAMES[p])
    }

    fn notes_of(st: &StepUi) -> String {
        let b = st.notes.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).trim().to_string()
    }

    /// The 2px outline colour and heading tick track the live/saved/cleared
    /// answer; a cleared answer removes both.
    fn mark_step(&self, i: usize) {
        let st = &self.steps[i];
        for c in ["guide-step-worked", "guide-step-broken", "guide-step-skipped"] {
            st.frame.remove_css_class(c);
        }
        let ns = n_str(&st.n);
        let base = format!("<b>{}. {}</b>", ns, glib::markup_escape_text(&st.head_text));
        match self.verdict_of(i) {
            Some("Worked") => {
                st.frame.add_css_class("guide-step-worked");
                st.head.set_markup(&format!("{base} \u{2713}"));
            }
            Some("Broken") => {
                st.frame.add_css_class("guide-step-broken");
                st.head.set_markup(&format!("{base} \u{2713}"));
            }
            Some("Skipped") => {
                st.frame.add_css_class("guide-step-skipped");
                st.head.set_markup(&format!("{base} \u{2713}"));
            }
            _ => st.head.set_markup(&base),
        }
    }

    fn results(&self, fill_skipped: bool) -> Value {
        Value::Array(
            (0..self.steps.len())
                .map(|i| {
                    let st = &self.steps[i];
                    let v = self.verdict_of(i).or(if fill_skipped { Some("Skipped") } else { None });
                    json!({"n": st.n, "verdict": v, "notes": Self::notes_of(st),
                           "shots": st.shots.borrow().iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()})
                })
                .collect(),
        )
    }

    fn say(&self, text: &str) {
        self.notice.set_text(text);
    }

    /// Ring the control a step names (or clear). Local tag rings the card's own
    /// back button, never the app.
    fn point(&self, idx: Option<usize>) {
        let (tag, local) = match idx {
            Some(i) => {
                let t = self.steps[i].tag.clone();
                let l = t == self.payload["local_tag"].as_str().unwrap_or("take-me-back-start");
                (t, l)
            }
            None => (String::new(), false),
        };
        if local {
            self.back.add_css_class("guide-card-ring");
        } else {
            self.back.remove_css_class("guide-card-ring");
        }
        let ok = if !tag.is_empty() && !local {
            with_sitting(&self.cell, |s| s.show_me_where(&tag)).unwrap_or(false)
        } else {
            with_sitting(&self.cell, |s| s.clear_marks());
            true
        };
        self.marked.set(if ok { idx } else { None });
        if !ok {
            let why = with_sitting(&self.cell, |s| s.error.clone()).unwrap_or_default();
            self.say(&why);
        }
        for (m, st) in self.steps.iter().enumerate() {
            if let Some(b) = &st.show {
                b.set_label(if idx == Some(m) && ok {
                    if local { "Showing here, not in the app" } else { "Showing in the app" }
                } else {
                    "Show me where"
                });
            }
        }
    }

    fn advance(&self, i: usize) {
        for m in i + 1..self.steps.len() {
            if !self.steps[m].tag.is_empty() && self.verdict_of(m).is_none() {
                self.point(Some(m));
                return;
            }
        }
        self.point(None);
    }

    fn copy_text(&self, text: &str) {
        use std::io::Write as _;
        let custom = std::env::var("CHECKRUN_COPY_CMD").ok();
        let cmd = custom.clone().unwrap_or_else(|| "wl-copy".into());
        let mut parts = cmd.split_whitespace();
        let ok = parts.next().and_then(|prog| {
            let mut child = std::process::Command::new(prog)
                .args(parts)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()?;
            child.stdin.take()?.write_all(text.as_bytes()).ok()?;
            child.wait().ok().filter(|s| s.success())
        });
        if ok.is_none() {
            if custom.is_some() {
                self.say("Copy failed.");
            } else {
                self.window.clipboard().set_text(text);
            }
        }
    }

    fn grab(self: &Rc<Self>, i: usize) {
        let st = &self.steps[i];
        let dir = PathBuf::from(self.payload["shots_dir"].as_str().unwrap_or(""));
        if let Err(e) = fs::create_dir_all(&dir) {
            self.say(&format!("Screenshot failed: {e}"));
            return;
        }
        let out = dir.join(format!("step{}-{}.png", n_str(&st.n), st.shots.borrow().len() + 1));
        let cmd = env_or("CHECKRUN_SHOT_CMD", "spectacle -r -b -n -o {out}").replace("{out}", &out.to_string_lossy());
        self.window.set_visible(false);
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let mut parts = cmd.split_whitespace();
            let ok = parts
                .next()
                .map(|p| std::process::Command::new(p).args(parts).status().map(|s| s.success()).unwrap_or(false))
                .unwrap_or(false);
            let _ = tx.send(ok);
        });
        let ui = self.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            res => {
                ui.window.set_visible(true);
                // Showing again can land the card anywhere: put it back.
                with_sitting(&ui.cell, |s| s.parent_geometry_changed());
                if res == Ok(true) && out.exists() {
                    ui.add_shot(i, out.clone());
                } else {
                    ui.say("Screenshot failed.");
                }
                glib::ControlFlow::Break
            }
        });
    }

    fn add_shot(self: &Rc<Self>, i: usize, path: PathBuf) {
        let st = &self.steps[i];
        st.shots.borrow_mut().push(path.clone());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&label(&path.file_name().unwrap_or_default().to_string_lossy(), false));
        let rm = gtk::Button::with_label("✕");
        rm.set_widget_name(&format!("guide-card-remove-shot-{}-{}", n_str(&st.n), st.shots.borrow().len() - 1));
        row.append(&rm);
        st.shot_rows.append(&row);
        st.remove_buttons.borrow_mut().push(rm.clone());
        let ui = self.clone();
        rm.connect_clicked(move |_| {
            let st = &ui.steps[i];
            st.shots.borrow_mut().retain(|p| p != &path);
            let _ = fs::remove_file(&path);
            st.shot_rows.remove(&row);
        });
    }

    fn finish(self: &Rc<Self>, action: &str) {
        if self.busy.get() {
            return;
        }
        let missing: Vec<String> = (0..self.steps.len())
            .filter(|&i| self.verdict_of(i).is_none())
            .map(|i| n_str(&self.steps[i].n))
            .collect();
        if action == "submit" && !missing.is_empty() && !self.confirm_skip.get() {
            self.confirm_skip.set(true);
            self.say(&format!(
                "Step{} {} not answered yet. Press Submit again to mark {} Skipped and write anyway.",
                if missing.len() > 1 { "s" } else { "" },
                missing.join(", "),
                if missing.len() > 1 { "them" } else { "it" }
            ));
            return;
        }
        let results = self.results(action == "submit");
        let began = with_sitting(&self.cell, |s| {
            if s.begin(action, &results) { Ok(()) } else { Err(s.error.clone()) }
        });
        match began {
            Some(Ok(())) => {
                self.busy.set(true);
                self.submit.set_sensitive(false);
                self.wait(action.to_string());
            }
            Some(Err(e)) => self.say(&e),
            None => {}
        }
    }

    fn press_back(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let began = with_sitting(&self.cell, |s| {
            if s.begin("back", &json!([])) { Ok(()) } else { Err(s.error.clone()) }
        });
        match began {
            Some(Ok(())) => {
                self.busy.set(true);
                self.wait("back".into());
            }
            Some(Err(e)) => self.say(&e),
            None => {}
        }
    }

    fn wait(self: &Rc<Self>, action: String) {
        let ui = self.clone();
        glib::timeout_add_local(Duration::from_millis(30), move || {
            let r = with_sitting(&ui.cell, |s| (s.poll(), s.error.clone()));
            let Some((r, err)) = r else { return glib::ControlFlow::Continue };
            let Some(ok) = r else { return glib::ControlFlow::Continue };
            ui.busy.set(false);
            if action == "back" {
                let says = ui.payload["way_back"]["says"].as_str().unwrap_or("").to_string();
                ui.back_says.set_text(if ok { "Putting it back. Give it a few seconds." } else { &err });
                if ok {
                    let ui2 = ui.clone();
                    glib::timeout_add_local_once(Duration::from_secs(6), move || ui2.back_says.set_text(&says));
                }
            } else if !ok {
                ui.submit.set_sensitive(true);
                ui.say(&err);
            }
            glib::ControlFlow::Break
        });
    }
}

fn install_card_css() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    if let Some(display) = gdk::Display::default() {
        let p = gtk::CssProvider::new();
        p.load_from_string(CARD_CSS);
        gtk::style_context_add_provider_for_display(&display, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn build_card(parent: &adw::ApplicationWindow, payload: &Value, cell: &SittingCell) -> Rc<CardUi> {
    install_card_css();
    let window = gtk::Window::builder()
        .title(payload["title"].as_str().unwrap_or("Checklist"))
        .default_width(CARD_W)
        .default_height(CARD_H)
        .build();
    let _ = parent; // placement follows the app; the card is never its WM transient
    window.set_widget_name("guide-card");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let detach = gtk::Button::with_label("Detach");
    detach.set_widget_name("guide-card-detach");
    let back = gtk::Button::with_label("Take me back to the start");
    back.set_widget_name("guide-card-back");
    back.set_sensitive(payload["way_back"]["available"] == Value::Bool(true));
    top.append(&detach);
    top.append(&back);
    root.append(&top);
    let back_says = label(payload["way_back"]["says"].as_str().unwrap_or(""), true);
    back_says.set_widget_name("guide-card-back-says");
    let notice = label(payload["notice"].as_str().unwrap_or(""), true);
    notice.set_widget_name("guide-card-notice");
    let duration = label(payload["duration"].as_str().unwrap_or(""), true);
    duration.set_widget_name("guide-card-duration");
    root.append(&back_says);
    root.append(&notice);
    root.append(&duration);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let mut steps = Vec::new();
    let saved = payload["saved"].as_object().cloned().unwrap_or_default();
    for step in payload["steps"].as_array().cloned().unwrap_or_default() {
        let n = step["n"].clone();
        let ns = n_str(&n);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 4);
        frame.set_widget_name(&format!("guide-card-step-{ns}"));
        let head = gtk::Label::new(None);
        head.set_xalign(0.0);
        head.set_wrap(true);
        let head_text = step["do"].as_str().unwrap_or("").to_string();
        head.set_widget_name(&format!("guide-card-head-{ns}"));
        head.set_markup(&format!("<b>{}. {}</b>", ns, glib::markup_escape_text(&head_text)));
        frame.append(&head);
        let hops = gtk::Grid::new();
        hops.set_column_spacing(8);
        for (r, hop) in step["hops"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let verb = gtk::Label::new(None);
            verb.set_xalign(0.0);
            verb.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(hop[0].as_str().unwrap_or(""))));
            hops.attach(&verb, 0, r as i32, 1, 1);
            let rest = label(hop[1].as_str().unwrap_or(""), true);
            rest.set_hexpand(true);
            hops.attach(&rest, 1, r as i32, 1, 1);
        }
        frame.append(&hops);
        let tag = step["tag"].as_str().unwrap_or("").to_string();
        let show = if tag.is_empty() {
            None
        } else {
            let b = gtk::Button::with_label("Show me where");
            b.set_widget_name(&format!("guide-card-show-{ns}"));
            b.set_halign(gtk::Align::Start);
            frame.append(&b);
            Some(b)
        };
        for (word, key, color) in [("Works", "works", "#3fb950"), ("Broken", "broken", "#f0883e")] {
            let t = step[key].as_str().unwrap_or("");
            if !t.is_empty() {
                let l = gtk::Label::new(None);
                l.set_xalign(0.0);
                l.set_wrap(true);
                l.set_markup(&format!(
                    "<b><span foreground=\"{color}\">{word}:</span></b> {}",
                    glib::markup_escape_text(t)
                ));
                l.set_widget_name(&format!("guide-card-says-{key}-{ns}"));
                frame.append(&l);
            }
        }
        let copy_texts: Vec<String> = step["copy"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|v| v.as_str().map(str::to_owned)).collect();
        let mut copy_buttons = Vec::new();
        for (ci, t) in copy_texts.iter().enumerate() {
            let b = gtk::Button::with_label(&format!("Copy “{t}”"));
            b.set_widget_name(&format!("guide-card-copy-{ns}-{ci}"));
            b.set_halign(gtk::Align::Start);
            frame.append(&b);
            copy_buttons.push(b);
        }
        let choices = gtk::Box::new(gtk::Orientation::Vertical, 2);
        choices.set_widget_name(&format!("guide-card-choices-{ns}"));
        let mut toggles: Vec<gtk::CheckButton> = Vec::new();
        for v in VERDICT_NAMES.iter() {
            let t = gtk::CheckButton::with_label(v);
            t.set_widget_name(&format!("guide-card-{}-{ns}", v.to_lowercase()));
            t.set_halign(gtk::Align::Start);
            if let Some(first) = toggles.first() {
                t.set_group(Some(first));
            }
            choices.append(&t);
            toggles.push(t);
        }
        // A grouped radio can only be switched ON by clicking; GTK never lets
        // a click turn the active one back off. The locked contract requires
        // exactly that (repeat-click clears the step), so a capture-phase
        // click gesture claims the press on the already-active control and
        // clears it directly before the widget's own click handling runs.
        for t in &toggles {
            let gesture = gtk::GestureClick::new();
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            let tt = t.clone();
            gesture.connect_pressed(move |g, _n, _x, _y| {
                if tt.is_active() {
                    tt.set_active(false);
                    g.set_state(gtk::EventSequenceState::Claimed);
                }
            });
            t.add_controller(gesture);
        }
        frame.append(&choices);
        let notes = gtk::TextView::new();
        notes.set_wrap_mode(gtk::WrapMode::WordChar);
        notes.set_widget_name(&format!("guide-card-notes-{ns}"));
        notes.set_height_request(48);
        let notes_overlay = gtk::Overlay::new();
        notes_overlay.set_child(Some(&notes));
        let notes_placeholder = gtk::Label::new(Some(NOTES_PLACEHOLDER));
        notes_placeholder.set_widget_name(&format!("guide-card-notes-placeholder-{ns}"));
        notes_placeholder.set_xalign(0.0);
        notes_placeholder.set_valign(gtk::Align::Start);
        notes_placeholder.set_halign(gtk::Align::Start);
        notes_placeholder.set_margin_top(4);
        notes_placeholder.set_margin_start(4);
        notes_placeholder.add_css_class("dim-label");
        notes_placeholder.set_can_target(false);
        notes_overlay.add_overlay(&notes_placeholder);
        {
            let ph = notes_placeholder.clone();
            notes.buffer().connect_changed(move |b| {
                ph.set_visible(b.text(&b.start_iter(), &b.end_iter(), false).is_empty());
            });
        }
        frame.append(&notes_overlay);
        let copy_notes = gtk::Button::with_label("Copy notes");
        copy_notes.set_widget_name(&format!("guide-card-copy-notes-{ns}"));
        copy_notes.set_halign(gtk::Align::Start);
        let grab = gtk::Button::with_label("Grab screenshot");
        grab.set_widget_name(&format!("guide-card-grab-{ns}"));
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tools.append(&copy_notes);
        tools.append(&grab);
        frame.append(&tools);
        let shot_rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
        frame.append(&shot_rows);
        let already = step["already"].as_str().unwrap_or("");
        let locked = !already.is_empty();
        if locked {
            frame.append(&label(already, true));
            for t in &toggles {
                t.set_sensitive(false);
            }
            toggles[0].set_active(true);
            notes.set_sensitive(false);
            grab.set_sensitive(false);
        } else if let Some(s) = saved.get(&ns) {
            if let Some(p) = s["verdict"].as_str().and_then(|v| VERDICT_NAMES.iter().position(|x| *x == v)) {
                toggles[p].set_active(true);
            }
            if let Some(t) = s["notes"].as_str() {
                notes.buffer().set_text(t);
            }
        }
        list.append(&frame);
        steps.push(StepUi {
            n, tag, show, toggles, copy_buttons, notes, notes_placeholder, copy_notes, grab, shot_rows,
            shots: RefCell::new(Vec::new()), remove_buttons: RefCell::new(Vec::new()),
            frame, head, head_text,
            copy_texts, was: RefCell::new(String::new()), locked, guard: Cell::new(false),
        });
    }
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let save = gtk::Button::with_label("Save for later");
    save.set_widget_name("guide-card-save");
    let submit = gtk::Button::with_label("Submit");
    submit.set_widget_name("guide-card-submit");
    bottom.append(&save);
    bottom.append(&submit);
    // CARD_H is the whole card's mapped-height budget, not an allowance for
    // the scroller alone. Capping only the scroller's own natural height
    // (the earlier, rejected fix) still let the fixed chrome above it — top
    // controls, back/notice/duration, margins — plus the fixed Save/Submit
    // row below it stack on top of a full CARD_H-tall scroller, so the real
    // two-step card still mapped at 1012 client px against a 640 px CARD_H.
    // Measure the chrome already built into `root` and the bottom row's own
    // natural height before either the scroller or the bottom row is
    // attached, then give the scroller only what's left of CARD_H, so the
    // WHOLE window — chrome, scroller and bottom row together — totals
    // CARD_H instead of CARD_H being added on top of the chrome.
    let (_, chrome_nat, _, _) = root.measure(gtk::Orientation::Vertical, CARD_W);
    let (_, bottom_nat, _, _) = bottom.measure(gtk::Orientation::Vertical, CARD_W);
    let spacing_reserve = root.spacing() * 2; // the two gaps root adds for the scroll and bottom rows
    let scroll_cap = (CARD_H - chrome_nat - bottom_nat - spacing_reserve).max(80);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(scroll_cap);
    scroll.set_child(Some(&list));
    root.append(&scroll);
    root.append(&bottom);
    window.set_child(Some(&root));

    let ui = Rc::new(CardUi {
        window, payload: payload.clone(), steps, detach, back, back_says, notice, save, submit,
        marked: Cell::new(None), busy: Cell::new(false), building: Cell::new(true),
        confirm_skip: Cell::new(false), attached: Cell::new(true), cell: cell.clone(),
    });
    for (i, st) in ui.steps.iter().enumerate() {
        if let Some(b) = &st.show {
            let u = ui.clone();
            b.connect_clicked(move |_| u.point(Some(i)));
        }
        for (ci, b) in st.copy_buttons.iter().enumerate() {
            let u = ui.clone();
            b.connect_clicked(move |_| u.copy_text(&u.steps[i].copy_texts[ci]));
        }
        let u = ui.clone();
        st.copy_notes.connect_clicked(move |_| u.copy_text(&CardUi::notes_of(&u.steps[i])));
        let u = ui.clone();
        st.grab.connect_clicked(move |_| u.grab(i));
        if st.locked {
            continue;
        }
        for (ti, t) in st.toggles.iter().enumerate() {
            let u = ui.clone();
            t.connect_toggled(move |t| {
                let st = &u.steps[i];
                if st.guard.get() {
                    return;
                }
                if t.is_active() {
                    st.guard.set(true);
                    for (oi, o) in st.toggles.iter().enumerate() {
                        if oi != ti {
                            o.set_active(false);
                        }
                    }
                    st.guard.set(false);
                    *st.was.borrow_mut() = VERDICT_NAMES[ti].into();
                    u.mark_step(i);
                    if !u.building.get() {
                        u.advance(i);
                    }
                } else {
                    st.was.borrow_mut().clear();
                    u.mark_step(i);
                }
            });
        }
    }
    for i in 0..ui.steps.len() {
        ui.mark_step(i);
    }
    ui.building.set(false);
    let u = ui.clone();
    ui.detach.connect_clicked(move |_| {
        let ok = if u.attached.get() {
            with_sitting(&u.cell, |s| s.detach())
        } else {
            with_sitting(&u.cell, |s| s.reattach())
        };
        if ok == Some(true) {
            u.attached.set(!u.attached.get());
            u.detach.set_label(if u.attached.get() { "Detach" } else { "Reattach" });
        } else {
            let e = with_sitting(&u.cell, |s| s.error.clone()).unwrap_or_default();
            u.say(&e);
        }
    });
    let u = ui.clone();
    ui.back.connect_clicked(move |_| u.press_back());
    let u = ui.clone();
    ui.save.connect_clicked(move |_| u.finish("save"));
    let u = ui.clone();
    ui.submit.connect_clicked(move |_| u.finish("submit"));
    ui
}

pub struct GtkAdapter {
    state: Weak<RefCell<AppState>>,
    window: adw::ApplicationWindow,
    payload: Value,
    pub card: Option<Rc<CardUi>>,
    poll: Option<glib::SourceId>,
    /// Parent geometry the current `desired` seam was computed from.
    placed_for: Option<(i32, i32, i32, i32)>,
    /// Desired card client origin (x, y) for `placed_for`. Reaching it is
    /// confirmed only by observing the card's own geometry, never assumed
    /// from having sent the move.
    desired: Option<(i32, i32)>,
    /// Consecutive `place_card_left` calls since `desired` last changed
    /// where the card's observed origin still disagreed with it.
    stall: u32,
    /// Set once `stall` has exhausted `MAX_PLACE_STALL` for the current
    /// `desired` seam and `place_card_left` has returned the terminal `Err`
    /// for it. Stops `needs_place` from asking for the same refused axis
    /// again (an endless move storm) while still having let that one `Err`
    /// surface through `parent_geometry_changed` instead of being lost.
    refused: bool,
    cell: SittingCell,
}

impl GtkAdapter {
    fn stop_poll(&mut self) {
        if let Some(id) = self.poll.take() {
            id.remove();
        }
    }

    /// True when the parent moved/resized since the last placement request,
    /// or the card's own observed origin still disagrees with the seam that
    /// request asked for and the mismatch has not yet been bounded off as a
    /// permanent refusal.
    pub fn needs_place(&self) -> bool {
        let Some(g) = window_geometry(&self.window) else { return false };
        if self.placed_for != Some(g) {
            return true;
        }
        if self.refused {
            return false;
        }
        let Some(desired) = self.desired else { return false };
        match self.card.as_ref().and_then(|c| window_geometry(&c.window)) {
            Some((ox, oy, _, _)) => (ox, oy) != desired,
            None => false,
        }
    }
}

/// The card is placed with X11 calls only; any other backend is refused.
pub fn backend_is_x11(display_type_name: &str) -> bool {
    display_type_name.contains("X11")
}

impl Adapter for GtkAdapter {
    fn create_card(&mut self, _app_key: &str) -> Result<String, String> {
        let display = gdk::Display::default().ok_or("no display")?;
        if !backend_is_x11(display.type_().name()) {
            return Err(format!("unsupported display backend {}: exact left placement needs X11", display.type_().name()));
        }
        let ui = build_card(&self.window, &self.payload, &self.cell);
        let cell = self.cell.clone();
        ui.window.connect_close_request(move |_| {
            with_sitting(&cell, |s| s.window_closed());
            glib::Propagation::Proceed
        });
        let cell = self.cell.clone();
        self.window.connect_close_request(move |_| {
            with_sitting(&cell, |s| s.window_closed());
            glib::Propagation::Proceed
        });
        ui.window.present();
        self.card = Some(ui);
        Ok("card".into())
    }

    fn place_card_left(&mut self, _app_key: &str, _card_key: &str) -> Result<(), String> {
        let ui = self.card.as_ref().ok_or("no card")?;
        let g = window_geometry(&self.window).ok_or("parent geometry unavailable")?;
        let card_w = if ui.window.width() > 0 { ui.window.width() } else { CARD_W };
        let desired = (g.0 - card_w, g.1);
        if self.placed_for != Some(g) || self.desired != Some(desired) {
            self.placed_for = Some(g);
            self.desired = Some(desired);
            self.stall = 0;
            self.refused = false;
        }
        let observed = window_geometry(&ui.window).map(|(x, y, _, _)| (x, y));
        if observed == Some(desired) {
            self.stall = 0;
            return Ok(());
        }
        // A sent XMoveWindow is not acknowledgement: only an observed match
        // clears the stall count. Past the bound the axis is a permanent
        // refusal, not silence: surface it as a terminal `Err` carrying the
        // exact observed/desired coordinates so the caller can record the
        // sitting truthfully, and stop re-issuing the move rather than
        // storming it every 40 ms or claiming `Ok` for a move that never
        // actually landed.
        if self.stall >= MAX_PLACE_STALL {
            self.refused = true;
            return Err(format!(
                "card observed origin {observed:?} still does not match desired {desired:?} after {MAX_PLACE_STALL} polls"
            ));
        }
        let (d, xid) = x11_handle(&ui.window).ok_or("card has no X11 surface")?;
        unsafe {
            XMoveWindow(d, xid, desired.0, desired.1);
            XFlush(d);
        }
        self.stall += 1;
        Ok(())
    }

    fn couple(&mut self, _app_key: &str, _card_key: &str) -> Result<(), String> {
        if self.card.is_none() {
            return Err("no card".into());
        }
        let cell = self.cell.clone();
        self.poll = Some(glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            with_sitting(&cell, |s| {
                if s.adapter.needs_place() {
                    s.parent_geometry_changed();
                }
            });
            glib::ControlFlow::Continue
        }));
        Ok(())
    }

    fn uncouple(&mut self, _card_key: &str) -> Result<(), String> {
        self.card.as_ref().ok_or("no card")?;
        self.stop_poll();
        Ok(())
    }

    fn mark(&mut self, _app_key: &str, control: &str) -> Result<(), String> {
        let state = self.state.upgrade().ok_or("app state gone")?;
        let known = serde_json::from_str::<Value>(super::GUIDE_REGISTRY)
            .ok()
            .and_then(|r| r.as_object().map(|o| o.contains_key(control)))
            .unwrap_or(false);
        if !known {
            return Err(format!("{control} is not in the guide registry"));
        }
        guide_apply(&state, GuideRequest::Known { name: control.into(), label: String::new() });
        Ok(())
    }

    fn clear_marks(&mut self, _app_key: &str) -> Result<(), String> {
        let state = self.state.upgrade().ok_or("app state gone")?;
        guide_apply(&state, GuideRequest::Clear);
        Ok(())
    }

    fn close_card(&mut self, _card_key: &str) -> Result<(), String> {
        self.stop_poll();
        if let Some(ui) = self.card.take() {
            ui.window.destroy();
        }
        Ok(())
    }

    fn reset_follow(&mut self, _app_key: &str, _card_key: &str) {
        self.stall = 0;
        self.refused = false;
    }

    fn close_app(&mut self, _app_key: &str) -> Result<(), String> {
        self.stop_poll();
        let state = self.state.upgrade().ok_or("app state gone")?;
        match ui_for_state(&state) {
            // Same state, but its registered window is no longer this exact
            // window: never unregister or close some other window instead.
            Some(ui) if ui.window != self.window => {
                return Err("bound window identity changed".into());
            }
            _ => {}
        }
        // Drops the explicit app.hold() so the launcher process can exit.
        // Idempotent: a window already closed by the user is already gone
        // from the registry, and this call proceeds to close it anyway.
        unregister_window(&state);
        self.window.close();
        Ok(())
    }
}

thread_local! {
    static LIVE_SITTING: RefCell<Vec<Rc<RefCell<Sitting<GtkAdapter>>>>> = const { RefCell::new(Vec::new()) };
}

/// The live sittings of this process (test seam).
#[cfg(test)]
#[allow(dead_code)]
pub fn live_sittings() -> Vec<Rc<RefCell<Sitting<GtkAdapter>>>> {
    LIVE_SITTING.with(|l| l.borrow().clone())
}

/// Builds and opens the sitting once checkrun has published its session
/// payload, for the window that claimed the descriptor token only. A decoy
/// never gets a card. No session file = an ordinary guide launch, nothing
/// happens.
pub fn start(state: &Rc<RefCell<AppState>>, window_key: usize) {
    let Some(token) = guide_claimed_by(window_key) else { return };
    let Some(descriptor) = guide_descriptor_path() else { return };
    let Some(ui) = ui_for_state(state) else { return };
    let window = ui.window.clone();
    let weak_state = Rc::downgrade(state);
    let started = Rc::new(Cell::new(false));
    window.connect_map(move |window| {
        if started.replace(true) {
            return;
        }
        let weak_win = window.downgrade();
        let weak_state = weak_state.clone();
        let descriptor = descriptor.clone();
        let token = token.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(window) = weak_win.upgrade() else { return glib::ControlFlow::Break };
            let Ok(text) = fs::read_to_string(side_path(&descriptor, "session")) else {
                return glib::ControlFlow::Continue;
            };
            let Ok(payload) = serde_json::from_str::<Value>(&text) else {
                return glib::ControlFlow::Continue; // half-written: try again
            };
            if payload["schema"] != "checkrun-card/1" {
                return glib::ControlFlow::Break;
            }
            let request_path = super::guide_read_descriptor(&descriptor)
                .and_then(|d| d["request_path"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let cell: SittingCell = Rc::new(RefCell::new(None));
            let adapter = GtkAdapter {
                state: weak_state.clone(),
                window: window.clone(),
                payload: payload.clone(),
                card: None,
                poll: None,
                placed_for: None,
                desired: None,
                stall: 0,
                refused: false,
                cell: cell.clone(),
            };
            let checklist = payload["checklist"].clone();
            let (d2, t2, c2) = (descriptor.clone(), token.clone(), checklist.clone());
            let verify = Box::new(move || run_verifier(&d2, &request_path, &c2, &t2));
            let t3 = token.clone();
            let claimed = Box::new(move |k: &str| {
                k.parse::<usize>().ok().and_then(guide_claimed_by).as_deref() == Some(t3.as_str())
            });
            let sitting = Rc::new(RefCell::new(Sitting::new(
                adapter,
                descriptor.clone(),
                token.clone(),
                window_key.to_string(),
                checklist,
                payload["answers_path"].as_str().unwrap_or("").into(),
                payload["progress_path"].as_str().unwrap_or("").into(),
                verify,
                claimed,
            )));
            *cell.borrow_mut() = Some(Rc::downgrade(&sitting));
            let opened = sitting.borrow_mut().open();
            if opened.is_ok() {
                LIVE_SITTING.with(|l| l.borrow_mut().push(sitting));
            }
            glib::ControlFlow::Break
        });
    });
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    type Log = Rc<RefCell<Vec<String>>>;

    /// What really exists, as opposed to what was called.
    #[derive(Default)]
    struct World {
        open: Vec<String>,
        coupled: bool,
        follows: usize,
    }

    struct Fake {
        world: Rc<RefCell<World>>,
        log: Log,
        fail: Rc<RefCell<Vec<&'static str>>>,
    }
    impl Fake {
        fn step(&self, op: &'static str, arg: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("{op}:{arg}"));
            if self.fail.borrow().contains(&op) { Err(format!("{op} failed")) } else { Ok(()) }
        }
    }
    impl Adapter for Fake {
        fn create_card(&mut self, a: &str) -> Result<String, String> {
            self.step("create", a)?;
            self.world.borrow_mut().open.push("card".into());
            Ok("card".into())
        }
        fn place_card_left(&mut self, _: &str, c: &str) -> Result<(), String> {
            self.step("place", c)?;
            self.world.borrow_mut().follows += 1;
            Ok(())
        }
        fn couple(&mut self, _: &str, c: &str) -> Result<(), String> {
            self.step("couple", c)?;
            self.world.borrow_mut().coupled = true;
            Ok(())
        }
        fn uncouple(&mut self, c: &str) -> Result<(), String> {
            self.step("uncouple", c)?;
            self.world.borrow_mut().coupled = false;
            Ok(())
        }
        fn mark(&mut self, a: &str, c: &str) -> Result<(), String> { self.step("mark", &format!("{a}/{c}")) }
        fn clear_marks(&mut self, a: &str) -> Result<(), String> { self.step("clear", a) }
        fn close_card(&mut self, c: &str) -> Result<(), String> {
            self.step("close_card", c)?;
            self.world.borrow_mut().open.retain(|w| w != "card");
            Ok(())
        }
        fn close_app(&mut self, a: &str) -> Result<(), String> {
            self.step("close_app", a)?;
            self.world.borrow_mut().open.retain(|w| w != "app");
            Ok(())
        }
    }

    /// Test copy under ~/Projects/Scratch, removed on pass, failure and panic.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Rig {
        _guard: Scratch,
        world: Rc<RefCell<World>>,
        dir: PathBuf,
        log: Log,
        fail: Rc<RefCell<Vec<&'static str>>>,
        ok: Rc<Cell<bool>>,
        claimed: Rc<Cell<bool>>,
    }

    fn tmp(name: &str) -> PathBuf {
        let d = crate::native_core::scratch_root().join(format!("gs154-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn rig(name: &str) -> (Rig, Sitting<Fake>) {
        let dir = tmp(name);
        let checklist = dir.join("list.md");
        fs::write(&checklist, "1. Do it\n").unwrap();
        let ident = json!({"path": checklist.to_string_lossy(), "sha256": file_sha256(&checklist).unwrap()});
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let fail = Rc::new(RefCell::new(Vec::new()));
        let ok = Rc::new(Cell::new(true));
        let claimed = Rc::new(Cell::new(true));
        let (o, c) = (ok.clone(), claimed.clone());
        let world = Rc::new(RefCell::new(World { open: vec!["app".into()], ..World::default() }));
        let s = Sitting::new(
            Fake { world: world.clone(), log: log.clone(), fail: fail.clone() },
            dir.join("desc.json"),
            "tok".into(),
            "app".into(),
            ident,
            dir.join("list_ANSWERS.md").to_string_lossy().into(),
            dir.join("list_PROGRESS.md").to_string_lossy().into(),
            Box::new(move || if o.get() { Ok(()) } else { Err("stale ack".into()) }),
            Box::new(move |_| c.get()),
        );
        (Rig { _guard: Scratch(dir.clone()), world, dir, log, fail, ok, claimed }, s)
    }

    fn record(r: &Rig) -> Value {
        serde_json::from_str(&fs::read_to_string(sitting_path(&r.dir.join("desc.json"))).unwrap()).unwrap()
    }

    fn calls(r: &Rig, op: &str) -> usize {
        r.log.borrow().iter().filter(|l| l.starts_with(&format!("{op}:"))).count()
    }

    fn reply(r: &Rig, nonce: &str, path: &Path, ok: bool, sha: Option<String>) {
        fs::write(
            side_path(&r.dir.join("desc.json"), "reply"),
            json!({"nonce": nonce, "ok": ok, "answers_path": path.to_string_lossy(),
                   "sha256": sha.or_else(|| file_sha256(path)), "error": if ok {""} else {"nope"}}).to_string(),
        )
        .unwrap();
    }

    fn nonce(r: &Rig) -> String {
        let t = fs::read_to_string(side_path(&r.dir.join("desc.json"), "request")).unwrap();
        serde_json::from_str::<Value>(&t).unwrap()["nonce"].as_str().unwrap().to_string()
    }

    fn no_temp_files(r: &Rig) -> bool {
        fs::read_dir(&r.dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".tmp"))
    }

    #[test]
    fn open_records_a_private_compositor_free_sitting() {
        use std::os::unix::fs::PermissionsExt;
        let (r, mut s) = rig("open");
        s.open().unwrap();
        let rec = record(&r);
        assert_eq!(rec["lifecycle"], "open");
        assert_eq!(rec["state"], "attached");
        assert_eq!(rec["token"], "tok");
        assert!(rec["checklist"]["sha256"].is_string());
        assert_eq!(fs::metadata(sitting_path(&r.dir.join("desc.json"))).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!rec.to_string().contains("x11") && !rec.to_string().contains("xid"));
    }

    #[test]
    fn refuses_unclaimed_stale_or_edited_and_creates_nothing() {
        for case in ["claim", "stale", "edit"] {
            let (r, mut s) = rig(case);
            match case {
                "claim" => r.claimed.set(false),
                "stale" => r.ok.set(false),
                _ => fs::write(r.dir.join("list.md"), "1. Changed\n").unwrap(),
            }
            assert!(s.open().is_err(), "{case}");
            assert_eq!(calls(&r, "create"), 0, "{case}");
            assert_eq!(record(&r)["lifecycle"], "refused", "{case}");
        }
    }

    #[test]
    fn every_attach_failure_refuses_and_tears_the_card_down() {
        for op in ["create", "place", "couple"] {
            let (r, mut s) = rig(op);
            r.fail.borrow_mut().push(op);
            assert!(s.open().is_err(), "{op}");
            assert_eq!(record(&r)["lifecycle"], "refused", "{op}");
            assert_eq!(calls(&r, "close_card"), if op == "create" { 0 } else { 1 }, "{op}");
            assert!(!s.is_open());
        }
    }

    #[test]
    fn unwritable_record_at_open_refuses_and_closes_the_card() {
        let (r, mut s) = rig("norecord");
        fs::create_dir(sitting_path(&r.dir.join("desc.json"))).unwrap(); // rename onto a dir fails
        assert!(s.open().is_err());
        assert_eq!(calls(&r, "close_card"), 1);
        assert!(!s.is_open() && no_temp_files(&r));
    }

    #[test]
    fn detach_happens_once_and_a_failed_detach_or_record_changes_nothing() {
        let (r, mut s) = rig("detach");
        s.open().unwrap();
        r.fail.borrow_mut().push("uncouple");
        assert!(!s.detach());
        assert!(s.is_attached());
        r.fail.borrow_mut().clear();
        let rec_path = sitting_path(&r.dir.join("desc.json"));
        let keep = fs::read_to_string(&rec_path).unwrap();
        fs::remove_file(&rec_path).unwrap();
        fs::create_dir(&rec_path).unwrap();
        assert!(!s.detach());
        assert!(s.is_attached(), "record failed: state must not move");
        fs::remove_dir(&rec_path).unwrap();
        fs::write(&rec_path, keep).unwrap();
        assert!(s.detach());
        assert!(!s.detach());
        assert_eq!(record(&r)["state"], "detached");
        assert_eq!(calls(&r, "uncouple"), 3);
    }

    #[test]
    fn reattach_restores_follow_to_the_same_window_and_detach_works_again() {
        let (r, mut s) = rig("reattach");
        s.open().unwrap();
        assert!(s.detach());
        assert!(!s.is_attached());
        let follows_before = r.world.borrow().follows;
        assert!(s.reattach());
        assert!(s.is_attached());
        assert_eq!(record(&r)["state"], "attached");
        assert!(r.world.borrow().coupled);
        assert_eq!(r.world.borrow().follows, follows_before + 1, "exactly one place on reattach");
        assert_eq!(calls(&r, "couple"), 2, "one poller started, not stacked");
        // Detach works again after a successful reattach.
        assert!(s.detach());
        assert!(!s.is_attached());
    }

    #[test]
    fn reattach_refuses_while_attached_or_before_open() {
        let (r, mut s) = rig("reattach-not-detached");
        assert!(!s.reattach(), "never open");
        s.open().unwrap();
        assert!(!s.reattach(), "already attached");
        assert!(s.is_attached());
        assert_eq!(calls(&r, "couple"), 1, "no extra poller from a refused reattach");
    }

    #[test]
    fn reattach_refuses_closed_and_creates_no_second_card() {
        let (r, mut s) = rig("reattach-closed");
        s.open().unwrap();
        assert!(s.detach());
        submit_ok(&r, &mut s);
        assert_eq!(s.poll(), Some(true));
        assert!(!s.is_open());
        assert!(!s.reattach(), "sitting is closed");
        assert_eq!(calls(&r, "create"), 1, "never a second card");
    }

    #[test]
    fn reattach_refuses_when_the_app_no_longer_owns_the_token() {
        let (r, mut s) = rig("reattach-gone");
        s.open().unwrap();
        assert!(s.detach());
        r.claimed.set(false);
        assert!(!s.reattach());
        assert!(!s.is_attached(), "state matches reality: still detached");
        assert!(!r.world.borrow().coupled, "no decoy coupling");
    }

    #[test]
    fn reattach_refuses_when_the_ack_is_stale() {
        let (r, mut s) = rig("reattach-stale");
        s.open().unwrap();
        assert!(s.detach());
        r.ok.set(false);
        assert!(!s.reattach());
        assert!(!s.is_attached());
        assert!(!r.world.borrow().coupled);
    }

    #[test]
    fn reattach_place_couple_or_record_failure_fails_closed_with_no_duplicate_poller() {
        for op in ["place", "couple"] {
            let (r, mut s) = rig(&format!("reattach-fail-{op}"));
            s.open().unwrap();
            assert!(s.detach());
            r.fail.borrow_mut().push(op);
            assert!(!s.reattach(), "{op}");
            assert!(!s.is_attached(), "{op}");
            assert_eq!(record(&r)["state"], "detached", "{op}");
            assert!(!r.world.borrow().coupled, "{op}: no half-coupled state");
        }
        // Record failure: coupling succeeded but the write did not hold, so it
        // must be undone rather than left ahead of the record.
        let (r, mut s) = rig("reattach-fail-record");
        s.open().unwrap();
        assert!(s.detach());
        let (p, keep) = break_record(&r);
        assert!(!s.reattach());
        assert!(!s.is_attached());
        assert!(!r.world.borrow().coupled, "coupling undone when the record would not hold");
        fix_record(&p, &keep);
        assert_eq!(record(&r)["state"], "detached");
    }

    fn submit_ok(r: &Rig, s: &mut Sitting<Fake>) {
        assert!(s.begin("submit", &json!([])));
        fs::write(r.dir.join("list_ANSWERS.md"), "answers").unwrap();
        reply(r, &nonce(r), &r.dir.join("list_ANSWERS.md"), true, None);
    }

    #[test]
    fn submit_records_first_then_closes_both() {
        let (r, mut s) = rig("submit");
        s.open().unwrap();
        submit_ok(&r, &mut s);
        assert_eq!(s.poll(), Some(true));
        assert_eq!(record(&r)["lifecycle"], "submitted");
        assert_eq!((calls(&r, "close_card"), calls(&r, "close_app")), (1, 1));
        assert!(no_temp_files(&r));
    }

    #[test]
    fn a_wrong_file_wrong_hash_stale_nonce_or_refusal_closes_nothing() {
        let (r, mut s) = rig("bad");
        s.open().unwrap();
        assert!(s.begin("submit", &json!([])));
        reply(&r, "stale-nonce", &r.dir.join("list_ANSWERS.md"), true, None);
        assert_eq!(s.poll(), None, "stale nonce is not an answer");
        let n = nonce(&r);
        fs::write(r.dir.join("other.md"), "x").unwrap();
        reply(&r, &n, &r.dir.join("other.md"), true, None);
        assert_eq!(s.poll(), Some(false));
        assert!(s.begin("submit", &json!([])));
        fs::write(r.dir.join("list_ANSWERS.md"), "answers").unwrap();
        reply(&r, &nonce(&r), &r.dir.join("list_ANSWERS.md"), true, Some("00".into()));
        assert_eq!(s.poll(), Some(false));
        assert!(s.begin("submit", &json!([])));
        reply(&r, &nonce(&r), &r.dir.join("list_ANSWERS.md"), false, None);
        assert_eq!(s.poll(), Some(false));
        assert_eq!(s.error, "nope");
        assert_eq!((calls(&r, "close_card"), calls(&r, "close_app")), (0, 0));
        assert!(s.is_open());
    }

    #[test]
    fn checkrun_silence_times_out_without_closing() {
        let (r, mut s) = rig("timeout");
        s.open().unwrap();
        s.set_timeout(Duration::from_millis(1));
        assert!(s.begin("submit", &json!([])));
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(s.poll(), Some(false));
        assert!(s.is_open() && calls(&r, "close_app") == 0);
    }

    #[test]
    fn stale_ack_or_edited_checklist_refuses_begin_and_show_me_where() {
        let (r, mut s) = rig("stale-gate");
        s.open().unwrap();
        r.ok.set(false);
        assert!(!s.begin("submit", &json!([])));
        assert!(!s.show_me_where("sidebar"));
        r.ok.set(true);
        assert!(s.show_me_where("sidebar"));
        fs::write(r.dir.join("list.md"), "1. Changed\n").unwrap();
        assert!(!s.begin("submit", &json!([])));
        r.claimed.set(false);
        assert!(!s.show_me_where("sidebar"));
        assert_eq!(calls(&r, "mark"), 1);
    }

    #[test]
    fn record_failure_after_the_reply_keeps_both_windows_open() {
        let (r, mut s) = rig("recfail");
        s.open().unwrap();
        submit_ok(&r, &mut s);
        let rec = sitting_path(&r.dir.join("desc.json"));
        fs::remove_file(&rec).unwrap();
        fs::create_dir(&rec).unwrap();
        assert_eq!(s.poll(), Some(false));
        assert!(s.is_open());
        assert_eq!((calls(&r, "close_card"), calls(&r, "close_app")), (0, 0));
        assert!(no_temp_files(&r));
    }

    #[test]
    fn a_close_failure_is_reported_never_success() {
        let (r, mut s) = rig("closefail");
        s.open().unwrap();
        r.fail.borrow_mut().push("close_app");
        submit_ok(&r, &mut s);
        assert_eq!(s.poll(), Some(false));
        assert!(s.error.contains("close_app failed"));
        assert_eq!(calls(&r, "close_card"), 1, "the other window is still closed");
    }

    #[test]
    fn closing_either_window_outside_submit_abandons_and_closes_both() {
        let (r, mut s) = rig("abandon");
        s.open().unwrap();
        s.window_closed();
        assert_eq!(record(&r)["lifecycle"], "abandoned");
        assert_eq!((calls(&r, "close_card"), calls(&r, "close_app")), (1, 1));
        s.window_closed();
        assert_eq!(calls(&r, "close_app"), 1, "idempotent");
    }

    fn break_record(r: &Rig) -> (PathBuf, String) {
        let p = sitting_path(&r.dir.join("desc.json"));
        let keep = fs::read_to_string(&p).unwrap();
        fs::remove_file(&p).unwrap();
        fs::create_dir(&p).unwrap();
        (p, keep)
    }

    fn fix_record(p: &Path, keep: &str) {
        fs::remove_dir(p).unwrap();
        fs::write(p, keep).unwrap();
    }

    #[test]
    fn detach_record_failure_restores_coupling_and_the_card_follows_again() {
        let (r, mut s) = rig("detach-rollback");
        s.open().unwrap();
        assert!(r.world.borrow().coupled);
        let (p, keep) = break_record(&r);
        assert!(!s.detach());
        assert!(s.is_attached());
        assert!(r.world.borrow().coupled, "coupling put back");
        assert!(s.error.contains("not written"));
        fix_record(&p, &keep);
        let before = r.world.borrow().follows;
        assert!(s.parent_geometry_changed());
        assert_eq!(r.world.borrow().follows, before + 1);
        assert_eq!(record(&r)["state"], "attached");
    }

    #[test]
    fn detach_record_and_recouple_both_failing_state_matches_reality() {
        let (r, mut s) = rig("detach-both");
        s.open().unwrap();
        break_record(&r);
        r.fail.borrow_mut().push("couple");
        assert!(!s.detach());
        assert!(!r.world.borrow().coupled);
        assert_eq!(s.state.as_deref(), Some("detached"));
    }

    #[test]
    fn abandon_record_failure_closes_nothing_and_does_not_claim_abandoned() {
        let (r, mut s) = rig("abandon-norec");
        s.open().unwrap();
        let (p, keep) = break_record(&r);
        assert!(!s.window_closed());
        assert!(s.is_open());
        assert_eq!(r.world.borrow().open.len(), 2);
        assert!(s.error.contains("not written"));
        fix_record(&p, &keep);
        assert_ne!(record(&r)["lifecycle"], "abandoned");
    }

    #[test]
    fn close_failure_stays_tracked_then_retry_leaves_no_owned_window() {
        let (r, mut s) = rig("close-retry");
        s.open().unwrap();
        r.fail.borrow_mut().push("close_app");
        submit_ok(&r, &mut s);
        assert_eq!(s.poll(), Some(false));
        let rec = record(&r);
        assert_eq!(rec["lifecycle"], "closing");
        assert_eq!(rec["outcome"], "submitted");
        assert_eq!(rec["owned"], json!(["app"]));
        assert!(rec["error"].as_str().unwrap().contains("close failed"));
        assert_eq!(r.world.borrow().open, vec!["app".to_string()]);
        assert!(s.is_tracked());
        r.fail.borrow_mut().clear();
        assert!(s.retry_close());
        let rec = record(&r);
        assert_eq!(rec["lifecycle"], "submitted");
        assert_eq!(rec["owned"], json!([]));
        assert!(r.world.borrow().open.is_empty());
        assert_eq!(calls(&r, "close_card"), 1, "closed windows are not closed twice");
    }

    #[test]
    fn closing_record_failure_on_submit_closes_nothing() {
        let (r, mut s) = rig("submit-norec");
        s.open().unwrap();
        submit_ok(&r, &mut s);
        break_record(&r);
        assert_eq!(s.poll(), Some(false));
        assert_eq!(r.world.borrow().open.len(), 2);
    }

    #[test]
    fn scratch_fixture_is_outside_tmp_and_removed() {
        let path = {
            let (r, _s) = rig("selfcheck");
            let p = r.dir.clone();
            assert!(!p.starts_with("/tmp"));
            assert!(p.components().any(|c| c.as_os_str() == "Scratch"));
            assert!(p.exists());
            p
        };
        assert!(!path.exists());
    }

    /// Fail the record writes at these absolute call indexes (1 = first).
    fn fail_writes(s: &Sitting<Fake>, idx: &[usize]) {
        *s.fail_at.borrow_mut() = idx.to_vec();
    }

    #[test]
    fn detach_record_and_recouple_fail_then_record_is_reconciled() {
        let (r, mut s) = rig("recon-detach");
        s.open().unwrap(); // write 1
        fail_writes(&s, &[2, 3]); // detached write, then the error write
        r.fail.borrow_mut().push("couple");
        assert!(!s.detach());
        assert!(!r.world.borrow().coupled);
        assert_eq!(record(&r)["state"], "attached", "not yet writable");
        assert!(s.parent_geometry_changed()); // writable again
        let rec = record(&r);
        assert_eq!(rec["state"], "detached");
        assert!(rec["error"].as_str().unwrap().contains("re-coupling failed"));
        assert!(!r.world.borrow().coupled);
    }

    #[test]
    fn follow_success_stays_attached_and_records_it() {
        let (r, mut s) = rig("follow-ok");
        s.open().unwrap();
        let before = r.world.borrow().follows;
        assert!(s.parent_geometry_changed());
        assert_eq!(r.world.borrow().follows, before + 1);
        assert!(s.is_attached());
        assert_eq!(record(&r)["state"], "attached");
        assert_eq!(record(&r)["error"], "");
    }

    #[test]
    fn follow_terminal_refusal_moves_off_attached_with_exact_error_and_keeps_owned() {
        let (r, mut s) = rig("follow-refused");
        s.open().unwrap();
        r.fail.borrow_mut().push("place"); // the adapter's terminal Err after its own bounded stall
        assert!(!s.parent_geometry_changed());
        assert!(!s.is_attached(), "a terminal follow refusal must never stay recorded as attached");
        // The shared schema only knows attached/detached; there is no third
        // state, so this lands on the existing "detached" value.
        assert_eq!(s.state.as_deref(), Some("detached"));
        assert!(s.error.contains("follow failed"), "{}", s.error);
        assert!(!r.world.borrow().coupled, "coupling must stop, not keep re-driving a refused move");
        let rec = record(&r);
        assert_eq!(rec["state"], "detached");
        assert_eq!(rec["lifecycle"], "open");
        assert_eq!(rec["owned"], json!(["card", "app"]), "windows stay tracked for exact cleanup");
        assert!(rec["error"].as_str().unwrap().contains("follow failed"));
    }

    #[test]
    fn follow_terminal_refusal_record_write_failure_fails_closed_not_attached() {
        let (r, mut s) = rig("follow-refused-norec");
        s.open().unwrap(); // write 1
        fail_writes(&s, &[2]); // the detached-on-refusal write
        r.fail.borrow_mut().push("place");
        assert!(!s.parent_geometry_changed());
        // Memory must reflect the truth even though the write did not land —
        // never revert to "attached" and invent a success that never happened.
        assert!(!s.is_attached());
        assert_eq!(s.state.as_deref(), Some("detached"));
        assert_eq!(record(&r)["state"], "attached", "not yet writable");
        assert!(s.error.contains("follow failed"));
        // Reconciling once writes work again catches the record up to reality.
        fail_writes(&s, &[]);
        assert!(s.reconcile());
        assert_eq!(record(&r)["state"], "detached");
        assert_eq!(record(&r)["owned"], json!(["card", "app"]));
    }

    #[test]
    fn reattach_after_a_refusal_clears_it_and_succeeds_once_the_condition_lifts() {
        let (r, mut s) = rig("follow-refused-reattach");
        s.open().unwrap();
        r.fail.borrow_mut().push("place");
        assert!(!s.parent_geometry_changed());
        assert_eq!(s.state.as_deref(), Some("detached"));
        assert!(!r.world.borrow().coupled);
        // The refusal condition clears (e.g. the real adapter's bounded
        // retry would reset once the desired seam changes); Reattach must
        // not inherit the old refusal and must start a fresh bounded
        // acknowledgement instead of failing again on stale bookkeeping.
        r.fail.borrow_mut().retain(|op| *op != "place");
        assert!(s.reattach());
        assert!(s.is_attached());
        assert!(r.world.borrow().coupled);
        assert_eq!(record(&r)["state"], "attached");
        assert_eq!(record(&r)["owned"], json!(["card", "app"]));
    }

    #[test]
    fn final_outcome_write_fails_record_reconciled_and_retryable() {
        let (r, mut s) = rig("recon-final");
        s.open().unwrap(); // write 1
        submit_ok(&r, &mut s);
        fail_writes(&s, &[3]); // 2 = closing ok, 3 = final outcome
        assert_eq!(s.poll(), Some(false));
        assert!(r.world.borrow().open.is_empty());
        assert_eq!(s.lifecycle.as_deref(), Some("closing"));
        assert!(s.owned.is_empty());
        assert_eq!(record(&r)["owned"], json!(["card", "app"]), "stale on disk");
        assert!(s.retry_close());
        let rec = record(&r);
        assert_eq!(rec["lifecycle"], "submitted");
        assert_eq!(rec["owned"], json!([]));
        assert_eq!(rec["error"], "");
    }

    #[test]
    fn final_outcome_write_fails_twice_reconcile_still_closing_then_clean() {
        let (r, mut s) = rig("recon-final2");
        s.open().unwrap();
        submit_ok(&r, &mut s);
        fail_writes(&s, &[3, 4, 5]);
        assert_eq!(s.poll(), Some(false));
        assert!(!s.retry_close());
        assert!(s.reconcile());
        let rec = record(&r);
        assert_eq!(rec["lifecycle"], "closing");
        assert_eq!(rec["owned"], json!([]));
        assert!(s.retry_close());
        assert_eq!(record(&r)["lifecycle"], "submitted");
    }

    #[test]
    fn close_failure_then_retry_clears_the_resolved_error() {
        let (r, mut s) = rig("recon-clear");
        s.open().unwrap();
        r.fail.borrow_mut().push("close_app");
        submit_ok(&r, &mut s);
        assert_eq!(s.poll(), Some(false));
        assert!(record(&r)["error"].as_str().unwrap().contains("close failed"));
        r.fail.borrow_mut().clear();
        assert!(s.retry_close());
        let rec = record(&r);
        assert_eq!(
            (rec["lifecycle"].clone(), rec["outcome"].clone(), rec["owned"].clone(), rec["error"].clone()),
            (json!("submitted"), json!("submitted"), json!([]), json!(""))
        );
        assert_eq!(s.error, "");
    }
}
